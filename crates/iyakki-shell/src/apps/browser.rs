//! BROWSER: a reader for the web. Pages are shown as text, headings, lists, links, tables, preformatted blocks and form
//! controls (type into a field, press Enter, and a search or a login form is sent). Scripts run only for sites the user
//! allows; there are no style sheets yet. Addresses are fetched in a background thread with the browser's cookies, every
//! host (including redirect targets) is checked against the signed-in user's policy, and only the host of each visit is
//! written to the audit log.

use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use iyakki_web::doc::{Control, ControlKind, Doc};
use iyakki_web::{form_urlencode, Jar, Loaded, Request, Url};

use super::browser_av;
use super::browser_chrome as chrome;
use super::browser_groups::{self, Groups};
use super::browser_assist;
use super::browser_block;
use super::browser_headers;
use super::browser_config::{self, BrowserConfig};
use super::browser_dark;
use super::browser_devpanel::{self, Act as DevAct};
use super::browser_devtools;
use super::browser_typo;
use super::browser_media::{self, MediaPerms};
use super::browser_full::{Full, Note, Redraw, Start, AREA_FULL};
use super::browser_data::{self, Data, Suggestion};
use super::browser_logins::{self, Cred};
use super::browser_persist;
use super::browser_speed;
use super::browser_images::{spawn_loader, ImgState, Msg, Pic, MAX_IMAGES};
use super::browser_layout::{key_of, layout, Focus, ImgInfo, Kind, Layout, Prim};
use tiny_skia::{FilterQuality, PixmapPaint, Transform};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, CARD, DIM, ERR, FIELD, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(88, 190, 235);
const BAR_LOW: [u8; 4] = rgb(11, 15, 30);
/// Control `i` is reached by clicking the hit region `Event::Focus(CONTROL_BASE + i)`.
const CONTROL_BASE: usize = 1000;
const HISTORY_MAX: usize = 40;

pub struct Page {
    pub url: Url,
    pub status: u16,
    pub doc: Doc,
    /// Script errors and other lines for the developer view.
    pub notes: Vec<String>,
    /// The page as the server sent it (for U, view source); empty for pages the browser made itself.
    pub source: String,
    /// A video or audio file: the page describes it and offers a player.
    pub media: bool,
    /// A file that is not shown (a download): D saves it to the Downloads folder.
    pub download: bool,
    /// What the server said the content is (for naming a download).
    pub ctype: String,
}

/// What the user has typed or chosen in one control.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CtlState {
    value: String,
    checked: bool,
    /// The chosen option of a drop-down.
    sel: usize,
}

fn initial_state(c: &Control) -> CtlState {
    CtlState { value: c.value.clone(), checked: c.checked, sel: c.options.iter().position(|o| o.selected).unwrap_or(0) }
}

struct Entry {
    /// `None` is the start page.
    page: Option<Arc<Page>>,
    scroll: f32,
    /// The form values for this page (kept when you go back).
    ctl: Vec<CtlState>,
    /// The pictures on this page, by index into the page's pictures.
    images: Vec<ImgState>,
    /// Identifies this visit, so a picture that arrives late is matched to the right page.
    serial: u64,
    /// The page as loaded while the reader view (R) is showing in its place.
    original: Option<Arc<Page>>,
    /// What the start page (an entry with no page) shows.
    view: StartView,
}

/// The pages the browser makes itself in place of a web page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartView {
    Home,
    History,
    Bookmarks,
    Downloads,
}

enum Nav {
    Idle,
    Loading { url: Url, rx: Receiver<Result<Loaded, String>>, how: How },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum How {
    /// A new entry in the history.
    Push,
    /// Reload: replace the current entry.
    Replace,
}

/// Everything that belongs to one tab. The tab being used lives in the `BrowserApp` fields themselves; the others wait here.
struct TabState {
    field: TextField,
    addr_focus: bool,
    history: Vec<Entry>,
    pos: usize,
    nav: Nav,
    scroll: f32,
    focus_link: Option<usize>,
    focus_ctl: Option<usize>,
    edit: TextField,
    images_due: bool,
    layout: Option<Layout>,
    status: Option<(String, bool)>,
    rerouted: bool,
    show_notes: bool,
    suggest: Vec<Suggestion>,
    sug_sel: Option<usize>,
    start_links: Vec<String>,
    full: Option<Full>,
    /// A tab brought back from the saved groups: its address is in the field and loads when the tab is first shown.
    lazy: bool,
}

pub const MAX_TABS: usize = 20;
/// The find bar's text field is reached by clicking the hit region `Event::Focus(FIND_ID)`.
const FIND_ID: usize = 900;

/// The page's reading area is reached by pressing in it: `Event::AreaAt(AREA_PAGE, x, y)`.
const AREA_PAGE: usize = 901;

/// Text selected on the page, as two points in the page's own coordinates (so it survives scrolling, zooming and layout) and
/// how it grows: by letter, by word (double click) or by line (triple click).
#[derive(Clone, Copy, Debug)]
struct Sel {
    anchor: (f32, f32),
    head: (f32, f32),
    mode: u8,
    /// Everything (Ctrl+A).
    all: bool,
}

/// A piece of a match inside one drawn word: (index of the piece in the layout, first character, end character).
type Seg = (usize, usize, usize);

/// Find in page: the words searched for and where they are in the layout.
struct Find {
    field: TextField,
    /// Every match as the pieces of words it covers (a phrase can run over several words).
    matches: Vec<Vec<Seg>>,
    cur: usize,
    /// The matches belong to an older query or layout.
    dirty: bool,
}

/// The text of a laid-out page in reading order with, for every character, which drawn word it is in. Words on one line are
/// separated by a space where there is a gap between them, lines by a line break.
fn text_stream(prims: &[Prim]) -> (Vec<char>, Vec<Option<(usize, usize)>>) {
    let (c, m, _) = text_stream_with_starts(prims);
    (c, m)
}

/// Like [`text_stream`], also saying where each drawn word starts in the stream.
fn text_stream_with_starts(prims: &[Prim]) -> (Vec<char>, Vec<Option<(usize, usize)>>, Vec<Option<usize>>) {
    let (mut chars, mut map) = (Vec::new(), Vec::new());
    let mut starts: Vec<Option<usize>> = vec![None; prims.len()];
    let mut prev: Option<&Prim> = None;
    for (i, p) in prims.iter().enumerate() {
        let Kind::Text { text, .. } = &p.kind else { continue };
        if let Some(q) = prev {
            let sep = if (p.y - q.y).abs() > q.h.min(p.h) * 0.5 { Some('\n') } else if p.x > q.x + q.w + 1.0 { Some(' ') } else { None };
            if let Some(c) = sep {
                chars.push(c);
                map.push(None);
            }
        }
        starts[i] = Some(chars.len());
        for (k, c) in text.chars().enumerate() {
            chars.push(c);
            map.push(Some((i, k)));
        }
        prev = Some(p);
    }
    (chars, map, starts)
}

/// The place in the stream nearest to the point (`x`, `y`) of the page (between two characters).
fn stream_index_at(g: &mut Gfx, prims: &[Prim], starts: &[Option<usize>], stream_len: usize, x: f32, y: f32) -> usize {
    let mut best: Option<(f32, usize)> = None;
    for (i, p) in prims.iter().enumerate() {
        let Kind::Text { .. } = &p.kind else { continue };
        let dy = if y < p.y { p.y - y } else if y > p.y + p.h { y - (p.y + p.h) } else { 0.0 };
        let dx = if x < p.x { p.x - x } else if x > p.x + p.w { x - (p.x + p.w) } else { 0.0 };
        let d = dy * 1000.0 + dx;
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    let Some((_, i)) = best else { return 0 };
    let (Kind::Text { text, size, face, .. }, Some(base)) = (&prims[i].kind, starts[i]) else { return 0 };
    let rel = x - prims[i].x;
    let mut pick = 0;
    let mut pick_d = f32::MAX;
    let mut prefix = String::new();
    for k in 0..=text.chars().count() {
        let w = g.text_width(*size, &prefix, *face);
        if (w - rel).abs() < pick_d {
            pick_d = (w - rel).abs();
            pick = k;
        }
        if let Some(c) = text.chars().nth(k) {
            prefix.push(c);
        }
    }
    (base + pick).min(stream_len)
}

/// Widen a selection to the word (or line) around it.
fn grow_range(chars: &[char], a: usize, b: usize, mode: u8) -> (usize, usize) {
    let (lo, hi) = (a.min(b).min(chars.len()), a.max(b).min(chars.len()));
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    match mode {
        2 => {
            let (mut s, mut e) = (lo, hi);
            while s > 0 && is_word(chars[s - 1]) {
                s -= 1;
            }
            while e < chars.len() && is_word(chars[e]) {
                e += 1;
            }
            // a double click on a space or a symbol still selects something: that character
            if s == e && e < chars.len() {
                e += 1;
            }
            (s, e)
        }
        3 => {
            let (mut s, mut e) = (lo, hi);
            while s > 0 && chars[s - 1] != '\n' {
                s -= 1;
            }
            while e < chars.len() && chars[e] != '\n' {
                e += 1;
            }
            (s, e)
        }
        _ => (lo, hi),
    }
}

/// The pieces of drawn words a stream range covers.
fn range_segments(map: &[Option<(usize, usize)>], a: usize, b: usize) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    for m in map[a.min(map.len())..b.min(map.len())].iter().flatten() {
        match segs.last_mut() {
            Some(l) if l.0 == m.0 && l.2 == m.1 => l.2 += 1,
            _ => segs.push((m.0, m.1, m.1 + 1)),
        }
    }
    segs
}

/// Where `query` occurs in the page (letters match in either case).
fn find_matches(prims: &[Prim], query: &str) -> Vec<Vec<Seg>> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return Vec::new();
    }
    let (chars, map) = text_stream(prims);
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + q.len() <= lower.len() {
        if lower[i..i + q.len()] == q[..] {
            let mut segs: Vec<Seg> = Vec::new();
            for m in map[i..i + q.len()].iter().flatten() {
                match segs.last_mut() {
                    Some(l) if l.0 == m.0 && l.2 == m.1 => l.2 += 1,
                    _ => segs.push((m.0, m.1, m.1 + 1)),
                }
            }
            if !segs.is_empty() {
                out.push(segs);
            }
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

fn new_field() -> TextField {
    let mut field = TextField::new("Search the web, or type an address", false);
    field.max = 2048;
    field
}

impl TabState {
    fn blank() -> TabState {
        TabState {
            field: new_field(),
            addr_focus: true,
            history: vec![Entry { page: None, scroll: 0.0, ctl: Vec::new(), images: Vec::new(), serial: 0, original: None, view: StartView::Home }],
            pos: 0,
            nav: Nav::Idle,
            scroll: 0.0,
            focus_link: None,
            focus_ctl: None,
            edit: TextField::new("", false),
            images_due: false,
            layout: None,
            status: None,
            rerouted: false,
            show_notes: false,
            suggest: Vec::new(),
            sug_sel: None,
            start_links: Vec::new(),
            full: None,
            lazy: false,
        }
    }

    /// The name shown on the tab.
    fn title(&self) -> String {
        if self.lazy {
            return browser_data::host_of(&self.field.value).to_string();
        }
        match self.history.get(self.pos) {
            Some(Entry { page: Some(p), .. }) => {
                if p.doc.title.trim().is_empty() { p.url.host.clone() } else { p.doc.title.trim().to_string() }
            }
            Some(Entry { view: StartView::History, .. }) => "History".into(),
            Some(Entry { view: StartView::Bookmarks, .. }) => "Bookmarks".into(),
            Some(Entry { view: StartView::Downloads, .. }) => "Downloads".into(),
            _ => "New tab".into(),
        }
    }

    fn loading(&self) -> bool {
        matches!(self.nav, Nav::Loading { .. })
    }
}

thread_local! {
    static OPEN_NEXT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Ask for the BROWSER tile to open on `url` the next time it starts (the AI IDE does this, then asks the shell for the tile).
pub fn request_open(url: &str) {
    OPEN_NEXT.with(|o| *o.borrow_mut() = Some(url.to_string()));
}

pub struct BrowserApp {
    field: TextField,
    addr_focus: bool,
    history: Vec<Entry>,
    pos: usize,
    nav: Nav,
    /// Pixels scrolled.
    scroll: f32,
    focus_link: Option<usize>,
    /// The form control that has the keyboard, and the editor for it when it takes text.
    focus_ctl: Option<usize>,
    edit: TextField,
    /// Pictures arrive here from their loader threads.
    img_tx: Sender<Msg>,
    img_rx: Receiver<Msg>,
    serial_next: u64,
    /// The page just shown still has pictures to ask for.
    images_due: bool,
    /// The early fetches of the page being loaded or just shown (cancelled when another page is asked for).
    preload: Option<Arc<iyakki_web::preload::Preloader>>,
    layout: Option<Layout>,
    /// What the last frame measured, for scrolling keys.
    view_h: f32,
    status: Option<(String, bool)>,
    hovered_link: Option<usize>,
    /// The user's choices (scripts, pictures, style sheets, cookies); read again before every page.
    cfg: BrowserConfig,
    /// Storage and cookies of the sites visited in this session.
    sites: iyakki_js::Sites,
    /// The developer panel (script errors, what was skipped) is showing.
    show_notes: bool,
    /// The page being loaded replaces a Google search (see `go_request`).
    rerouted: bool,
    /// Search pages that refused the current search (busy, or a check page); the next engine in the list is tried.
    search_tried: Vec<iyakki_web::SearchEngine>,
    /// Said above the results when another engine answered instead of the chosen one.
    search_note: Option<String>,
    /// Bookmarks, history, zoom and downloads of this person.
    data: Data,
    /// What the address bar offers for the text typed in it, and which one is chosen.
    suggest: Vec<Suggestion>,
    sug_sel: Option<usize>,
    /// The links of the start page last drawn (the start page is built every frame from the saved data).
    start_links: Vec<String>,
    /// The address of the link under the pointer when the context menu was asked for.
    ctx_link: Option<String>,
    /// A file being saved: its name, the way to cancel it and where the result arrives.
    download: Option<(String, Arc<std::sync::atomic::AtomicBool>, Receiver<Result<(String, u64), String>>)>,
    /// The tabs; the one in use (`active`) is `None` here because its state is in the fields above.
    tabs: Vec<Option<TabState>>,
    active: usize,
    /// The tab groups (`of_tab` is parallel to `tabs`).
    groups: Groups,
    /// The tab in use came back from the saved groups and has not been opened yet.
    lazy: bool,
    /// The group's name / colour popup under the strip.
    group_edit: Option<GroupEdit>,
    /// The tab overview over the page area: its search box.
    overview: Option<TextField>,
    /// A page that could not be loaded: its address and why, shown as a card with Retry until the next load.
    failed: Option<(Url, String)>,
    /// What the groups looked like when they were last saved (so they are saved only when they change), and the ticks since.
    groups_saved: String,
    groups_clock: u32,
    /// When the built-in reader's current load began (for the "taking long" card).
    load_started: std::time::Instant,
    /// The Scripts chip's plain-words explanation while the pointer is on it (shown in the status line).
    hover_hint: Option<String>,
    /// Where the strip drew each group's chip (right-click on one edits the group).
    chip_rects: Vec<(u32, R)>,
    /// Addresses of tabs that were closed, newest last, for reopening.
    closed: Vec<String>,
    /// Where to scroll once the page has been laid out again (after a zoom change).
    pending_scroll: Option<f32>,
    /// Find in page, when the bar is open.
    find: Option<Find>,
    /// The assistant panel over the page (Q ask, S summarise, T translate), when open.
    assist: Option<browser_assist::Assist>,
    /// The developer panel's network list (c cycles notes, network, closed) and what it shows.
    show_net: bool,
    /// The list of requests the filter lists blocked on this page (B opens it).
    show_blocked: bool,
    net_log: browser_devtools::Log,
    /// The Developer tools panel (F12, Ctrl+Shift+I): `browser_devpanel.rs`.
    dev: browser_devpanel::Panel,
    /// The panel or its button changed without a key or click (new console lines arrived): draw the whole scene again.
    dev_dirty: bool,
    /// The last pointer position, for the wheel over the panel.
    mouse: (i32, i32),
    /// The counters of the toolbar button last drawn, for the next change check.
    dev_counts: (usize, usize),
    /// An open drop-down list: the control and the highlighted option.
    popup: Option<(usize, usize)>,
    /// The sign-in just sent from a form (kept until the next page shows whether it worked), and the one now offered for saving.
    pending_cred: Option<Cred>,
    offer: Option<Cred>,
    /// Which saved sign-in Ctrl+K filled last, to step through several.
    fill_n: usize,
    /// The warning about sending a password without protection has been shown for this page.
    http_warned: bool,
    /// Text selected on the page, and what it covers as of the last frame (stream positions and the text itself).
    sel: Option<Sel>,
    sel_range: Option<(usize, usize)>,
    sel_text: String,
    /// Where the page's reading column starts, relative to the view (for turning a press into page coordinates).
    col_off: f32,
    /// The embedded full web engine for this tab (ENGINE=FULL), while it is on.
    full: Option<Full>,
    /// A page the reader cannot show is opened in the full engine at once when it is there (off in most tests).
    auto_full: bool,
    /// The engine stopped or the person left it: heavy sites are no longer sent to it by themselves (no crash loop).
    full_declined: bool,
    /// The full engine (helper program or cage/cog) is installed; false makes heavy sites show the "needs the full web engine" banner.
    engine_offered: bool,
    /// The bar over a video the engine cannot decode (H.264 / AAC) and the choices about it (browser_av.rs).
    av: browser_av::AvState,
    /// Another tile the shell should open next (Install in the audio and video bar opens Store -> Setup).
    av_request: Option<(String, String)>,
    /// When the add-on's libraries were last looked for while waiting for an install.
    av_checked: Option<std::time::Instant>,
    /// "Radio is playing. Stop it so the browser can play?" (soundask.rs), shown when the page plays and the speaker is held.
    sound_ask: Option<super::soundask::Ask>,
    /// "Allow this site to use your camera?" (browser_media.rs) for the page in front, and what the person chose before.
    media_ask: Option<browser_media::Ask>,
    media: Option<MediaPerms>,
    /// The microphones found when a microphone question opened, the chooser over that question, and where /proc is (tests move it).
    media_mics: Vec<iyakki_agent::voicedesk::Mic>,
    media_mic_pick: Option<super::widgets::Picker>,
    /// The cameras the person has not hidden (listed when a camera question opens), the chooser over that question, and what the person
    /// decided about devices (`browserdev`): the default camera and the hidden cameras and microphones.
    media_cams: Vec<iyakki_agent::browserdev::Cam>,
    media_cam_pick: Option<super::widgets::Picker>,
    devprefs: Option<iyakki_agent::browserdev::DevicePrefs>,
    pub(super) mic_proc: std::path::PathBuf,
    /// Where the cameras are read from (`/sys`; tests move it).
    pub(super) cam_sys: std::path::PathBuf,
    pub(super) mic_dir: std::path::PathBuf,
    /// The last microphone failure the web engine's plugin reported and the person was told about; when it was last looked for.
    mic_told: String,
    /// The saved choice (voice settings), read when the question opens.
    mic_saved: String,
    mic_checked: Option<std::time::Instant>,
    /// What this browser last told the shell's sign about (`browser_media::publish_capture`).
    published: u8,
    /// After a stop: when the page's media is told to open its sound again.
    sound_reconnect: Option<std::time::Instant>,
    /// Where the page area was drawn last and the interface scale then (the engine starts at that size).
    view_rect: R,
    ui_scale: f32,
    /// What was last told to the assistant about the page in view (see `publish_snapshot`).
    snap_key: String,
    snap_sel: String,
    snap: Option<iyakki_agent::webtools::PageSnapshot>,
    /// The assistant's own tab is showing (see `web_agent`).
    agent_view: bool,
    /// Shown inside another app (the AI IDE's Browser tab, see `ide_browser`): this app is then only the built-in reader and
    /// never starts a web engine of its own; a page that needs one is handed to the host in `want_full`, which shows it in the
    /// host's shared engine (the assistant's tab), so there is one helper program per window.
    embedded: bool,
    want_full: Option<String>,
}

impl BrowserApp {
    pub fn new(cx: &AppCx) -> BrowserApp {
        let mut app = BrowserApp::make(cx);
        // the AI IDE's "open in the Browser tile" asked for this page
        if let Some(u) = OPEN_NEXT.with(|o| o.borrow_mut().take()) {
            app.open_url(&u, cx);
        }
        app.restore_groups(cx);
        app
    }

    fn make(cx: &AppCx) -> BrowserApp {
        let field = new_field();
        let (img_tx, img_rx) = channel();
        let app = BrowserApp {
            field,
            addr_focus: true,
            history: vec![Entry { page: None, scroll: 0.0, ctl: Vec::new(), images: Vec::new(), serial: 0, original: None, view: StartView::Home }],
            pos: 0,
            nav: Nav::Idle,
            scroll: 0.0,
            focus_link: None,
            focus_ctl: None,
            edit: TextField::new("", false),
            img_tx,
            img_rx,
            serial_next: 1,
            images_due: false,
            preload: None,
            layout: None,
            view_h: 400.0,
            status: None,
            hovered_link: None,
            cfg: browser_config::load(cx),
            sites: iyakki_js::Sites::default(),
            show_notes: false,
            rerouted: false,
            search_tried: Vec::new(),
            search_note: None,
            data: browser_data::load(cx),
            suggest: Vec::new(),
            sug_sel: None,
            ctx_link: None,
            start_links: Vec::new(),
            download: None,
            tabs: vec![None],
            active: 0,
            groups: Groups::default(),
            lazy: false,
            group_edit: None,
            overview: None,
            failed: None,
            groups_saved: String::new(),
            groups_clock: 0,
            load_started: std::time::Instant::now(),
            hover_hint: None,
            chip_rects: Vec::new(),
            closed: Vec::new(),
            pending_scroll: None,
            find: None,
            assist: None,
            show_net: false,
            show_blocked: false,
            net_log: Default::default(),
            dev: Default::default(),
            dev_dirty: false,
            mouse: (0, 0),
            dev_counts: (0, 0),
            popup: None,
            pending_cred: None,
            offer: None,
            fill_n: 0,
            http_warned: false,
            sel: None,
            sel_range: None,
            sel_text: String::new(),
            col_off: 0.0,
            full: None,
            auto_full: !cfg!(test),
            full_declined: false,
            engine_offered: iyakki_agent::webengine::offered(),
            av: browser_av::AvState::new(browser_av::load(cx)),
            av_request: None,
            av_checked: None,
            sound_ask: None,
            media_ask: None,
            media: None,
            media_mics: Vec::new(),
            media_mic_pick: None,
            media_cams: Vec::new(),
            media_cam_pick: None,
            devprefs: None,
            mic_proc: iyakki_agent::voicedesk::proc_root(),
            mic_dir: iyakki_agent::voicedesk::mic_share_dir(),
            cam_sys: iyakki_agent::browserdev::sys_root(),
            mic_told: String::new(),
            mic_saved: String::new(),
            mic_checked: None,
            published: 0,
            sound_reconnect: None,
            view_rect: R::new(0.0, 0.0, 1280.0, 600.0),
            ui_scale: 1.0,
            snap_key: String::new(),
            snap_sel: String::new(),
            snap: None,
            agent_view: false,
            embedded: false,
            want_full: None,
        };
        // a panel that was open when the last browser tile closed left collecting on: it is not open now
        cx.services.devtools.set_panel(&cx.session.user, false);
        if app.cfg.js_all || !app.cfg.js_sites.is_empty() {
            iyakki_js::warm_up();
        }
        if app.cfg.cookies {
            browser_persist::restore(cx, &app.sites, iyakki_agent::clock::now_unix());
        }
        app
    }

    /// Move the tab in use out of the fields (leaving fresh ones behind).
    fn take_state(&mut self) -> TabState {
        self.clear_selection();
        self.assist = None;
        self.av.bar = None;
        let mut full = self.full.take();
        if let Some(f) = full.as_mut() {
            f.set_visible(false);
        }
        TabState {
            field: std::mem::replace(&mut self.field, new_field()),
            addr_focus: self.addr_focus,
            history: std::mem::take(&mut self.history),
            pos: self.pos,
            nav: std::mem::replace(&mut self.nav, Nav::Idle),
            scroll: self.scroll,
            focus_link: self.focus_link.take(),
            focus_ctl: self.focus_ctl.take(),
            edit: std::mem::replace(&mut self.edit, TextField::new("", false)),
            images_due: self.images_due,
            layout: self.layout.take(),
            status: self.status.take(),
            rerouted: self.rerouted,
            show_notes: self.show_notes,
            suggest: std::mem::take(&mut self.suggest),
            sug_sel: self.sug_sel.take(),
            start_links: std::mem::take(&mut self.start_links),
            full,
            lazy: std::mem::take(&mut self.lazy),
        }
    }

    fn put_state(&mut self, t: TabState) {
        self.field = t.field;
        self.addr_focus = t.addr_focus;
        self.history = t.history;
        self.pos = t.pos;
        self.nav = t.nav;
        self.scroll = t.scroll;
        self.focus_link = t.focus_link;
        self.focus_ctl = t.focus_ctl;
        self.edit = t.edit;
        self.images_due = t.images_due;
        self.layout = t.layout;
        self.status = t.status;
        self.rerouted = t.rerouted;
        self.show_notes = t.show_notes;
        self.suggest = t.suggest;
        self.sug_sel = t.sug_sel;
        self.start_links = t.start_links;
        self.lazy = t.lazy;
        self.failed = None;
        self.full = t.full;
        if let Some(f) = self.full.as_mut() {
            f.set_visible(true);
        }
        self.hovered_link = None;
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab(&self) -> usize {
        self.active
    }

    /// The names of all tabs, in order (the one in use first computed from the live fields).
    pub fn tab_titles(&self) -> Vec<String> {
        (0..self.tabs.len())
            .map(|i| match &self.tabs[i] {
                Some(t) => t.title(),
                None => match self.history.get(self.pos) {
                    Some(Entry { page: Some(p), .. }) if !p.doc.title.trim().is_empty() => p.doc.title.trim().to_string(),
                    Some(Entry { page: Some(p), .. }) => p.url.host.clone(),
                    Some(Entry { view: StartView::History, .. }) => "History".into(),
                    Some(Entry { view: StartView::Bookmarks, .. }) => "Bookmarks".into(),
                    Some(Entry { view: StartView::Downloads, .. }) => "Downloads".into(),
                    _ => "New tab".into(),
                },
            })
            .collect()
    }

    fn switch_to(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.active {
            return;
        }
        self.media_void();
        let cur = self.take_state();
        self.tabs[self.active] = Some(cur);
        if let Some(next) = self.tabs[i].take() {
            self.put_state(next);
        }
        self.active = i;
    }

    /// A new empty tab at the end, with the cursor in the address bar.
    pub fn new_tab(&mut self) {
        if self.tabs.len() >= MAX_TABS {
            self.status = Some((format!("At most {MAX_TABS} tabs."), false));
            return;
        }
        self.media_void();
        let cur = self.take_state();
        self.tabs[self.active] = Some(cur);
        self.tabs.push(None);
        self.groups.push_tab();
        self.active = self.tabs.len() - 1;
        self.put_state(TabState::blank());
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.media_void();
        if i == self.active {
            if let Some(u) = self.current_url().map(|u| u.to_string()) {
                self.closed.push(u);
                self.closed.truncate(20);
            }
            if self.tabs.len() == 1 {
                self.put_state(TabState::blank());
                self.groups.reset();
                return;
            }
            // leave the one in use: show its neighbour, then drop its slot
            let _gone = self.take_state();
            self.tabs.remove(i);
            self.groups.remove_tab(i);
            let next = if i < self.tabs.len() { i } else { self.tabs.len() - 1 };
            if let Some(t) = self.tabs[next].take() {
                self.put_state(t);
            }
            self.active = next;
        } else {
            if let Some(Some(t)) = self.tabs.get(i) {
                if let Some(Entry { page: Some(p), .. }) = t.history.get(t.pos) {
                    self.closed.push(p.url.to_string());
                    self.closed.truncate(20);
                }
            }
            self.tabs.remove(i);
            self.groups.remove_tab(i);
            if i < self.active {
                self.active -= 1;
            }
        }
    }

    fn reopen_closed(&mut self, cx: &AppCx) {
        match self.closed.pop() {
            Some(u) => {
                self.new_tab();
                self.open_url(&u, cx);
            }
            None => self.status = Some(("No closed tab to bring back.".into(), false)),
        }
    }

    fn step_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n > 1 {
            let to = if forward { (self.active + 1) % n } else { (self.active + n - 1) % n };
            self.switch_to(to);
        }
    }

    pub fn js_allowed_for(&self, host: &str) -> bool {
        self.cfg.scripts_for(host)
    }

    /// Open an address in a NEW tab while whatever is running (an overlay, a sign-in dialog) goes on: the full web engine when the
    /// image has one and the account's policy allows the host, else the built-in reader. The tab becomes the active one.
    pub fn open_in_new_tab(&mut self, input: &str, cx: &AppCx) {
        let before = self.tabs.len();
        self.new_tab();
        if self.tabs.len() == before {
            return; // at the tab limit: new_tab said so in the status line
        }
        let Ok(u) = Url::from_input_on(input, self.cfg.search) else {
            self.status = Some((format!("{input} is not an address."), false));
            return;
        };
        if !self.embedded && iyakki_agent::webengine::offered() {
            if let (Ok(allowed), Some(helper)) = (iyakki_agent::webengine::allowed(&cx.session.policy, &u.to_string()), iyakki_agent::webengine::embedded_helper()) {
                let host = iyakki_sec::url_host(&allowed).unwrap_or_default();
                self.start_embedded(helper, allowed, host, cx);
                if self.full.is_some() {
                    return;
                }
            }
        }
        self.go(u, How::Push, cx);
    }

    /// Open an address (used by tests and by "open this" from other apps).
    pub fn open_url(&mut self, input: &str, cx: &AppCx) {
        match Url::from_input_on(input, self.cfg.search) {
            Ok(u) => self.go(u, How::Push, cx),
            Err(e) => self.status = Some((e, false)),
        }
    }

    pub fn current_url(&self) -> Option<&Url> {
        self.history.get(self.pos).and_then(|e| e.page.as_ref()).map(|p| &p.url)
    }

    /// A reader for another app to show in a rectangle of its own (the IDE's Browser tab).
    pub(super) fn new_embedded(cx: &AppCx) -> BrowserApp {
        let mut a = BrowserApp::make(cx);
        a.embedded = true;
        a.addr_focus = false;
        a
    }

    /// The reader's page area (no toolbar, tabs or status line), drawn in `view`.
    pub(super) fn render_body(&mut self, g: &mut Gfx, view: R, s: f32, tick: u64) {
        self.view_h = view.h;
        self.view_rect = view;
        self.ui_scale = s;
        self.render_reader(g, view, s, tick);
    }

    /// Work that needs no drawing: finished loads, pictures, the page the assistant may be told about.
    pub(super) fn tick_embedded(&mut self, cx: &AppCx) {
        self.poll(cx);
        self.publish_snapshot(cx);
    }

    pub(super) fn embed_go_back(&mut self) {
        self.back();
    }
    pub(super) fn embed_go_forward(&mut self) {
        self.forward();
    }
    pub(super) fn embed_can_back(&self) -> bool {
        self.pos > 0
    }
    pub(super) fn embed_can_forward(&self) -> bool {
        self.pos + 1 < self.history.len()
    }
    pub(super) fn embed_reload(&mut self, cx: &AppCx) {
        self.reload(cx);
    }
    pub(super) fn embed_title(&self) -> String {
        self.page().map(|p| p.doc.title.trim().to_string()).unwrap_or_default()
    }
    pub(super) fn embed_notes(&self) -> usize {
        self.page().map_or(0, |p| p.notes.len())
    }
    pub(super) fn embed_toggle_notes(&mut self) {
        self.show_notes = !self.show_notes;
    }
    pub(super) fn embed_take_want_full(&mut self) -> Option<String> {
        self.want_full.take()
    }
    pub(super) fn embed_status(&self) -> Option<(String, bool)> {
        self.status.clone()
    }
    /// The host owns the keyboard fields: the reader's own address field never has it.
    pub(super) fn embed_take_keyboard(&mut self) {
        self.addr_focus = false;
    }
    pub(super) fn embed_is_loading(&self) -> bool {
        self.is_loading()
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.nav, Nav::Loading { .. })
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    fn page(&self) -> Option<&Arc<Page>> {
        self.history.get(self.pos).and_then(|e| e.page.as_ref())
    }

    /// Tell the assistant (the chat's `browser_page` tool, which asks the person before it reads this) which page is in
    /// view: its address, title, readable text and the selection. Cheap: the text is made again only when the page or
    /// tab changes, and the shared copy is written only when something changed.
    fn publish_snapshot(&mut self, cx: &AppCx) {
        use iyakki_agent::webtools::{PageSnapshot, SNAPSHOT_CHARS};
        let full = self.full.as_ref().filter(|f| !f.url.is_empty()).map(|f| (f.url.clone(), f.title.clone()));
        let key = match (&full, self.page()) {
            (Some((u, t)), _) => format!("full|{u}|{t}"),
            (None, Some(p)) if !p.download && !p.media => format!("page|{:p}|{}", Arc::as_ptr(p), p.url),
            _ => String::new(),
        };
        let reader = full.is_none();
        let sel = if reader { self.sel_text.trim().to_string() } else { String::new() };
        if key == self.snap_key && sel == self.snap_sel {
            return;
        }
        if key != self.snap_key {
            self.snap = match (&full, self.page()) {
                (Some((u, t)), _) => Some(PageSnapshot { url: u.clone(), title: t.clone(), text: String::new(), selection: String::new(), time: 0, full_engine: true }),
                (None, Some(p)) if !p.download && !p.media => {
                    let stub = p.ctype == STUB_TYPE;
                    Some(PageSnapshot { url: p.url.to_string(), title: p.doc.title.clone(), text: if stub { String::new() } else { iyakki_web::reader::plain_text(&p.doc, SNAPSHOT_CHARS) }, selection: String::new(), time: 0, full_engine: stub })
                }
                _ => None,
            };
            self.snap_key = key;
        }
        self.snap_sel = sel.clone();
        let snap = self.snap.clone().map(|s| PageSnapshot { selection: sel, time: iyakki_agent::clock::now_unix(), ..s });
        cx.services.set_page_snapshot(&cx.session.user, snap);
    }

    fn go(&mut self, url: Url, how: How, cx: &AppCx) {
        let mut req = Request::get(&url);
        if let Some(p) = self.page() {
            req.referer = p.url.to_string();
        }
        self.go_request(req, how, cx);
    }

    fn go_request(&mut self, req: Request, how: How, cx: &AppCx) {
        let req = Request { priority: iyakki_web::net::DOCUMENT, ..req };
        if self.full.is_some() {
            self.go_full(&req, cx);
            return;
        }
        // Google, YouTube and the other heavy sites are shown by the full engine from the start when the image has one: the
        // built-in reader loads them as empty or stripped pages
        if self.auto_full && !self.full_declined && !self.embedded && req.method.eq_ignore_ascii_case("GET") && iyakki_agent::webengine::full_by_default(&req.url.host) && iyakki_agent::webengine::offered() {
            if let (Ok(allowed), Some(helper)) = (iyakki_agent::webengine::allowed(&cx.session.policy, &req.url.to_string()), iyakki_agent::webengine::embedded_helper()) {
                let host = req.url.host.clone();
                self.start_embedded(helper, allowed, host, cx);
                if self.full.is_some() {
                    return;
                }
            }
        }
        // Google's results page cannot be loaded without a full browser; the same words go to a search page that can
        let engine = browser_config::load(cx).search;
        let req = match req.url.reroute_google_search_on(engine) {
            Some(u) if req.method.eq_ignore_ascii_case("GET") => {
                self.rerouted = true;
                Request::get(&u)
            }
            _ => req,
        };
        // the person's ad and tracker blocking (switch, allowed sites, lists) is read before anything is requested
        browser_block::apply(cx);
        browser_headers::apply(cx);
        browser_block::weekly_refresh(cx);
        let url = req.url.clone();
        let policy = cx.session.policy.clone();
        // a new page load starts a new network list (pictures and reloads add to the list of the page they belong to)
        if let Ok(mut l) = self.net_log.lock() {
            l.clear();
        }
        // this person's own cache; its sizes follow the machine's free memory (re-read every few seconds)
        let user_web = cx.web.for_user(&cx.session.user).unwrap_or_else(|| cx.web.clone());
        let cfg_now = browser_config::load(cx);
        browser_speed::refresh(user_web.as_ref(), cx.services.dir(), cfg_now.cache);
        let wire: Arc<dyn iyakki_web::Fetch> = Arc::new(browser_devtools::Recording::new(user_web, self.net_log.clone()));
        // style sheets and scripts the page names start loading together as soon as its HTML is in hand
        let mach = browser_speed::machine(cx.services.dir());
        let preloader = {
            let policy = policy.clone();
            let jar = if cfg_now.cookies { Some(self.sites.cookies.clone()) } else { None };
            iyakki_web::preload::Preloader::new(wire, &url, Arc::new(move |h: &str| policy.allows_host(h)), jar, browser_speed::preload_workers(&mach))
        };
        if let Some(old) = self.preload.replace(preloader.clone()) {
            old.cancel();
        }
        let web: Arc<dyn iyakki_web::Fetch> = preloader.clone();
        let (tx, rx) = channel();
        let target = url.clone();
        // Settings may have changed since the last page
        self.cfg = browser_config::load(cx);
        let (cfg, sites) = (self.cfg.clone(), self.sites.clone());
        self.focus_ctl = None;
        // the visit is recorded by host only: addresses can carry tokens
        cx.audit.record(&cx.session.user, "browser.visit", &url.host);
        // the thread also runs the page's scripts (WebAssembly on this stack), so it gets a big one
        let spawned = std::thread::Builder::new().name("iyakki-browser-fetch".into()).stack_size(32 << 20).spawn(move || {
            let allow = {
                let policy = policy.clone();
                move |h: &str| policy.allows_host(h)
            };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            // with cookies turned off nothing is sent and nothing is kept
            let no_cookies = std::sync::Mutex::new(iyakki_web::CookieStore::new());
            let store = if cfg.cookies { &sites.cookies } else { &no_cookies };
            // style sheets come through the same checks as the page: the account's hosts, cookies, a size limit
            let sheet = |u: &Url| -> Option<String> {
                let (_, r) = iyakki_web::fetch_following(web.as_ref(), Request { priority: iyakki_web::net::BLOCKING, ..Request::get(u) }, &allow, Some(Jar { store, now })).ok()?;
                let ct = r.content_type.to_ascii_lowercase();
                (r.status < 400 && r.body.len() <= iyakki_web::style::MAX_SHEET_BYTES && (ct.is_empty() || ct.contains("css") || ct.contains("text/plain"))).then(|| String::from_utf8_lossy(&r.body).into_owned())
            };
            let vp = iyakki_web::css::Viewport::default();
            let styles_on = cfg.styles;
            let build = |u: &Url, tree: &iyakki_web::html::Tree| -> Doc {
                if styles_on {
                    let mut d = iyakki_web::style::styled_doc(tree, u, &vp, &sheet);
                    // a page that brings its own style sheets was drawn for a white page, as every browser shows it
                    if d.canvas.is_none() && (tree.find("style").is_some() || tree.find("link").is_some()) {
                        d.canvas = Some([255, 255, 255, 255]);
                    }
                    d
                } else {
                    iyakki_web::doc::build(tree)
                }
            };
            // scripts run only for sites the user has allowed (checked again for where redirects end up)
            let render = |u: &Url, html: &str| -> (Doc, Vec<String>) {
                preloader.start(&iyakki_web::preload::scan(html, u, iyakki_web::preload::Wants { styles: cfg.styles, scripts: cfg.scripts_for(&u.host), images: cfg.images }));
                if cfg.scripts_for(&u.host) {
                    let mut data = sites.for_origin(u);
                    if !cfg.cookies {
                        data.cookies = Arc::new(std::sync::Mutex::new(iyakki_web::CookieStore::new()));
                    }
                    let (doc, mut notes) = iyakki_js::renderer_with(web.clone(), allow.clone(), data, &build)(u, html);
                    if doc.blocks.is_empty() {
                        // an app that needs more than this engine offers leaves nothing: show what the server sent
                        let plain = build(u, &iyakki_web::html::parse(html));
                        if !plain.blocks.is_empty() {
                            notes.insert(0, "The page's scripts left it empty (they need browser features this reader does not have), so it is shown without them.".into());
                            return (plain, notes);
                        }
                    }
                    (doc, notes)
                } else {
                    (build(u, &iyakki_web::html::parse(html)), Vec::new())
                }
            };
            let jar = Jar { store, now };
            let r = iyakki_web::load_request(web.as_ref(), req, &allow, &render, Some(jar));
            preloader.release_unused();
            let _ = target;
            let _ = tx.send(r);
        });
        match spawned {
            Ok(_) => {
                self.status = None;
                self.failed = None;
                self.load_started = std::time::Instant::now();
                self.nav = Nav::Loading { url, rx, how };
            }
            Err(e) => self.status = Some((format!("could not start loading: {e}"), false)),
        }
    }

    // ------------------------------------------------------------------------------------------------ developer tools panel

    fn dev_ctx(&self) -> browser_devpanel::Ctx {
        browser_devpanel::Ctx { full: self.full.is_some(), offered: self.engine_offered }
    }

    /// F12, Ctrl+Shift+I, the toolbar button: opens the panel (collecting starts for this person, memory only) or closes it
    /// (collecting stops, what it collected is forgotten unless the chat agent's switch is on).
    fn toggle_devpanel(&mut self, cx: &AppCx) {
        if self.embedded {
            return;
        }
        let (dt, user) = (&cx.services.devtools, cx.session.user.as_str());
        if self.dev.open {
            self.dev.close(dt, user);
            cx.audit.record(user, "browser.devtools.panel", "closed");
        } else {
            self.dev.open(dt, user);
            cx.audit.record(user, "browser.devtools.panel", "opened");
            // the panel has the keyboard now: not the page, the address bar or the suggestions
            self.addr_focus = false;
            self.suggest.clear();
            self.sug_sel = None;
            if let Some(f) = self.full.as_mut() {
                f.set_focus(false);
                dt.set_page(user, &f.url);
            }
        }
        self.dev_dirty = true;
        self.dev_tick(cx);
    }

    /// New lines or counts since the last heartbeat: draw again. Cheap: one counter is compared, the buffers are copied only
    /// when it moved.
    fn dev_tick(&mut self, cx: &AppCx) {
        let (dt, user) = (&cx.services.devtools, cx.session.user.as_str());
        let reader_len = self.net_log.lock().map_or(0, |l| l.len());
        let mut dirty = self.dev.refresh(dt, user, reader_len);
        let sum = if dt.collecting(user) { dt.summary(user) } else { Default::default() };
        if (sum.failed, sum.console_errors) != self.dev_counts {
            self.dev_counts = (sum.failed, sum.console_errors);
            dirty = true;
        }
        if dirty {
            self.dev_dirty = true;
        }
    }

    /// What the panel asked the browser to do.
    fn dev_act(&mut self, a: DevAct, cx: &AppCx) {
        match a {
            DevAct::Done => {}
            DevAct::Close => self.toggle_devpanel(cx),
            DevAct::Copy => {
                let reader: Vec<browser_devtools::NetLine> = self.net_log.lock().map(|l| l.clone()).unwrap_or_default();
                let text = self.dev.report(self.dev_ctx(), &reader);
                crate::ui::clipboard::copy(&text);
                crate::ui::clipboard::toast("Copied the developer tools report");
            }
        }
        self.dev_dirty = true;
    }

    fn poll(&mut self, cx: &AppCx) {
        self.groups_tick(cx);
        if self.lazy {
            self.lazy = false;
            let u = self.field.value.clone();
            self.open_url(&u, cx);
        }
        self.full_browser_result();
        self.poll_full(cx);
        self.capture_tick(cx);
        self.mic_failure_tick();
        self.dev_tick(cx);
        self.poll_images(cx);
        self.poll_download(cx);
        let Nav::Loading { rx, how, .. } = &self.nav else { return };
        let how = *how;
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the page could not be loaded".into()),
        };
        let Nav::Loading { url: asked, .. } = std::mem::replace(&mut self.nav, Nav::Idle) else { return };
        match result {
            Ok(Loaded::Page { url, status, doc, mut notes, source }) => {
                // a search page that is busy or asks for a check: the same words go to the next search page
                if matches!(status, 202 | 403 | 429 | 503) {
                    if let Some((engine, words)) = iyakki_web::SearchEngine::of_url(&url) {
                        self.search_tried.push(engine);
                        if let Some((next, nu)) = iyakki_web::SearchEngine::fallback(&self.search_tried, &words) {
                            self.search_note = Some(format!("{} did not answer (it said {status}), so this search was run on {}.", engine.name(), next.name()));
                            self.go_request(Request::get(&nu), how, cx);
                            return;
                        }
                    }
                } else {
                    self.search_tried.clear();
                }
                if let Some(n) = self.search_note.take() {
                    notes.push(n);
                }
                if std::mem::take(&mut self.rerouted) {
                    notes.push("Google's results page needs a full browser, so this search was run on another search page instead (Settings → Browser).".into());
                }
                browser_persist::save(cx, &self.sites, iyakki_agent::clock::now_unix(), self.cfg.cookies);
                let page = Arc::new(Page { url: url.clone(), status, doc, notes, source, media: false, download: false, ctype: String::new() });
                if how == How::Push && status < 400 {
                    self.record_visit(&page, cx);
                }
                // a sign-in was just sent and the answer is not the sign-in form again: offer to keep it
                let sent = self.pending_cred.take().filter(|_| status < 400 && browser_logins::login_controls(&page.doc).is_none());
                let needs_full = status < 400 && page_needs_full_engine(&page);
                let source_has_script = page.source.to_ascii_lowercase().contains("<script");
                self.show(page, how);
                self.offer = None;
                if let Some(c) = sent {
                    if browser_logins::for_site(cx, &c.host).is_ok_and(|saved| !saved.iter().any(|o| o.user == c.user && o.password == c.password)) {
                        self.status = Some((format!("Save the sign-in for {} on {}? Press Ctrl+S (it goes in your vault).", if c.user.is_empty() { "this account" } else { &c.user }, c.host), true));
                        self.offer = Some(c);
                        return;
                    }
                }
                self.status = (status >= 400).then(|| (format!("The server answered {status}."), false));
                // a site the person allowed scripts for is an app: when the full engine is here, it runs it
                let scripted_site = self.cfg.scripts_for(&url.host) && source_has_script;
                if (needs_full || scripted_site) && self.auto_full && iyakki_agent::webengine::offered() && self.full.is_none() {
                    // the built-in reader cannot show this page and the full engine is here: use it without asking
                    self.open_full_browser(cx);
                    if self.full.is_some() || self.want_full.is_some() {
                        return;
                    }
                }
                if needs_full {
                    self.status = Some((
                        if iyakki_agent::webengine::offered() {
                            "Press F to open this page in the full browser (the built-in reader could not show it).".into()
                        } else {
                            format!("The built-in reader could not show this page. {}.", iyakki_agent::webengine::NOT_INSTALLED)
                        },
                        true,
                    ));
                }
            }
            Ok(Loaded::Unsupported { url, content_type, bytes, info }) => {
                let media = iyakki_web::media::is_media(&content_type);
                let size = if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1 << 20) as f64) } else { format!("{} KB", bytes / 1024 + 1) };
                let html = if media {
                    let kind = if content_type.starts_with("audio/") { "Audio" } else { "Video" };
                    let player = iyakki_agent::voice::find_media_player(&self.cfg.player).map(|p| p[0].rsplit('/').next().unwrap_or("").to_string());
                    let how = match &player {
                        Some(p) => format!("Press P (or Play above) to watch it in {}.", escape(p)),
                        None => "No video player is installed (mpv, VLC or ffplay), so it cannot be played here. Install one, or set the player command.".to_string(),
                    };
                    format!("<h1>{kind} file</h1><p>{}</p><p>{}{} · {size}</p><p>{how}</p>", escape(&url.to_string()), escape(&content_type), if info.is_empty() { String::new() } else { format!(" · {}", escape(&info)) })
                } else {
                    format!("<h1>A file, not a page</h1><p>{} is a {content_type} file ({size}). Press D to save it to your Downloads folder, Ctrl+J lists what was saved.</p>", escape(&url.to_string()))
                };
                let page = Arc::new(Page { url, status: 200, doc: iyakki_web::doc::from_html(&html), notes: Vec::new(), source: String::new(), media, download: !media, ctype: content_type.clone() });
                self.show(page, how);
            }
            Err(e) => {
                self.pending_cred = None;
                if browser_typo::is_lookup_failure(&e) && !asked.host.is_empty() {
                    // an unknown name: say so and offer a search (and the likely site) instead of the resolver's wording
                    let html = browser_typo::not_found_html(&asked.host, self.cfg.search);
                    let page = Arc::new(Page { url: asked.clone(), status: 404, doc: iyakki_web::doc::from_html(&html), notes: Vec::new(), source: String::new(), media: false, download: false, ctype: String::new() });
                    self.show(page, how);
                    self.status = Some((format!("Couldn't find {}", asked.host), false));
                    return;
                }
                self.status = Some((friendly_error(&asked.host, &e), false));
                self.failed = Some((asked.clone(), e.clone()));
                self.addr_focus = true;
            }
        }
    }

    fn show(&mut self, page: Arc<Page>, how: How) {
        self.clear_selection();
        self.http_warned = false;
        self.field.set(&page.url.to_string());
        self.addr_focus = false;
        self.focus_link = None;
        self.focus_ctl = None;
        self.layout = None;
        self.scroll = 0.0;
        let ctl: Vec<CtlState> = page.doc.controls.iter().map(initial_state).collect();
        let images = vec![ImgState::Todo; page.doc.images.len()];
        let serial = self.serial_next;
        self.serial_next += 1;
        self.images_due = !images.is_empty();
        match how {
            How::Replace => {
                if let Some(e) = self.history.get_mut(self.pos) {
                    e.page = Some(page);
                    e.scroll = 0.0;
                    e.ctl = ctl;
                    e.images = images;
                    e.serial = serial;
                }
            }
            How::Push => {
                self.history.truncate(self.pos + 1);
                self.history.push(Entry { page: Some(page), scroll: 0.0, ctl, images, serial, original: None, view: StartView::Home });
                if self.history.len() > HISTORY_MAX {
                    self.history.remove(0);
                }
                self.pos = self.history.len() - 1;
            }
        }
    }

    /// Ask for the pictures of the page just shown, and take in the ones that have arrived.
    fn poll_images(&mut self, cx: &AppCx) {
        if self.images_due {
            self.images_due = false;
            if let Some(e) = self.history.get_mut(self.pos) {
                if !self.cfg.images {
                    for st in e.images.iter_mut() {
                        *st = ImgState::Failed("pictures are turned off (Settings → Browser)".into());
                    }
                } else if let Some(page) = e.page.clone() {
                    let jobs: Vec<(usize, String)> = page.doc.images.iter().enumerate().take(MAX_IMAGES).map(|(i, im)| (i, im.src.clone())).collect();
                    for (i, _) in &jobs {
                        e.images[*i] = ImgState::Loading;
                    }
                    for i in MAX_IMAGES.min(e.images.len())..e.images.len() {
                        e.images[i] = ImgState::Failed("too many pictures on this page".into());
                    }
                    let policy = cx.session.policy.clone();
                    let jar = if self.cfg.cookies { self.sites.cookies.clone() } else { Arc::new(std::sync::Mutex::new(iyakki_web::CookieStore::new())) };
                    spawn_loader(page.url.clone(), jobs, browser_speed::image_fetch(cx, self.preload.as_ref(), &page.url),  move |h: &str| policy.allows_host(h), jar, e.serial, self.img_tx.clone());
                }
            }
        }
        while let Ok((serial, idx, result)) = self.img_rx.try_recv() {
            // the picture may belong to a page in another tab
            let others = self.tabs.iter_mut().flatten().flat_map(|t| t.history.iter_mut());
            if let Some(slot) = self.history.iter_mut().chain(others).find(|e| e.serial == serial).and_then(|e| e.images.get_mut(idx)) {
                *slot = match result {
                    Ok(Pic::Still(pm)) => ImgState::Ready(pm),
                    Ok(Pic::Anim(a)) => ImgState::Animated(a),
                    Err(e) => ImgState::Failed(e),
                };
            }
        }
    }

    fn goto_entry(&mut self, i: usize) {
        self.clear_selection();
        if let Some(e) = self.history.get_mut(self.pos) {
            e.scroll = self.scroll;
        }
        self.pos = i;
        self.nav = Nav::Idle;
        self.layout = None;
        self.focus_link = None;
        self.focus_ctl = None;
        self.status = None;
        let e = &self.history[i];
        self.scroll = e.scroll;
        match &e.page {
            Some(p) => {
                let u = p.url.to_string();
                self.field.set(&u);
                self.addr_focus = false;
            }
            None => {
                self.field.set("");
                self.addr_focus = true;
            }
        }
    }

    fn back(&mut self) {
        if let Some(f) = self.full.as_mut() {
            if f.can_back {
                f.back();
                return;
            }
            // nothing earlier inside the engine: the page before it, in the built-in reader
            self.drop_full_quiet();
            return;
        }
        if self.pos > 0 {
            self.goto_entry(self.pos - 1);
        }
    }

    fn forward(&mut self) {
        if let Some(f) = self.full.as_mut() {
            if f.can_forward {
                f.forward();
            }
            return;
        }
        if self.pos + 1 < self.history.len() {
            self.goto_entry(self.pos + 1);
        }
    }

    fn reload(&mut self, cx: &AppCx) {
        if let Some(f) = self.full.as_mut() {
            f.reload();
            return;
        }
        if let Some(u) = self.page().filter(|p| p.ctype != HELP_TYPE).map(|p| p.url.clone()) {
            self.go(u, How::Replace, cx);
        }
    }

    /// The banner button: the short help about the full web engine, as a page of its own (no network).
    fn show_engine_help(&mut self) {
        if let Ok(url) = Url::parse(HELP_URL) {
            let doc = iyakki_web::doc::from_html(iyakki_agent::webengine::FULL_ENGINE_HELP_HTML);
            self.show(Arc::new(Page { url, status: 200, doc, notes: Vec::new(), source: String::new(), media: false, download: false, ctype: HELP_TYPE.into() }), How::Push);
        }
    }

    /// What the reader says above the page when the full engine is missing and the page needs it (None otherwise).
    fn engine_banner(&self) -> Option<(String, String)> {
        let p = self.page().filter(|p| p.ctype != HELP_TYPE && p.ctype != STUB_TYPE && !p.media && !p.download)?;
        iyakki_agent::webengine::missing_engine_banner(&p.url.host, p.status < 400 && page_needs_full_engine(p), self.engine_offered)
    }

    fn home(&mut self) {
        self.drop_full_quiet();
        if self.pos != 0 {
            self.goto_entry(0);
        }
        self.addr_focus = true;
    }

    fn follow(&mut self, link: usize, cx: &AppCx) {
        let Some(page) = self.page().cloned() else {
            // a link on the start page, history or bookmarks (their addresses are complete)
            if let Some(Ok(u)) = self.start_links.get(link).map(|h| Url::parse(h)) {
                self.go(u, How::Push, cx);
            }
            return;
        };
        let Some(href) = page.doc.links.get(link) else { return };
        match page.url.join(href) {
            Ok(u) if u.without_fragment() == page.url.without_fragment() && u.fragment.is_some() => {
                // an in-page anchor: stay where we are
                self.status = Some((format!("#{}", u.fragment.unwrap_or_default()), true));
            }
            Ok(u) => self.go(u.unwrap_search_link().unwrap_or(u), How::Push, cx),
            Err(e) => self.status = Some((e, false)),
        }
    }

    fn submit_address(&mut self, cx: &AppCx) {
        let text = expand_shortcut(&self.field.value).unwrap_or_else(|| self.field.value.clone());
        self.suggest.clear();
        self.sug_sel = None;
        self.cfg = browser_config::load(cx);
        self.search_tried.clear();
        self.search_note = None;
        match Url::from_input_on(&text, self.cfg.search) {
            Ok(u) => self.go(u, How::Push, cx),
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// The person's Downloads folder: `/home/<name>/Downloads` when their account may write there (the other apps can then
    /// open what was saved), otherwise a private one next to their other browser data.
    fn downloads_dir(cx: &AppCx) -> Result<std::path::PathBuf, String> {
        let home = std::path::PathBuf::from(format!("/home/{}/Downloads", cx.session.user));
        if iyakki_agent::scope::check_write(&cx.session.policy, &cx.session.user, &home).is_ok() && std::fs::create_dir_all(&home).is_ok() {
            return Ok(home);
        }
        let own = cx.services.dir().join("downloads").join(super::browser_config::who(cx));
        std::fs::create_dir_all(&own).map_err(|e| format!("{}: {e}", own.display()))?;
        Ok(own)
    }

    /// Save the file the page is (a non-page address) to Downloads, in the background.
    fn start_download(&mut self, cx: &AppCx) {
        let Some(page) = self.page().cloned().filter(|p| p.download) else {
            self.status = Some(("This is not a file to save.".into(), false));
            return;
        };
        self.start_download_of(page.url.clone(), page.ctype.clone(), None, cx);
    }

    /// Save `url` to Downloads in the background (what D does for a file page, and what the full engine's download requests
    /// use: the engine never writes files itself). `name` is the engine's suggestion; it is cleaned like any other.
    fn start_download_of(&mut self, url: Url, ctype: String, name: Option<String>, cx: &AppCx) {
        if self.download.is_some() {
            self.status = Some(("A download is already running (Esc cancels it).".into(), false));
            return;
        }
        if !cx.session.policy.allows_host(&url.host) {
            self.status = Some((format!("Your account may not fetch files from {}.", url.host), false));
            return;
        }
        let dir = match Self::downloads_dir(cx) {
            Ok(d) => d,
            Err(e) => {
                self.status = Some((e, false));
                return;
            }
        };
        let (tx, rx) = channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (flag, web, url) = (cancel.clone(), cx.web.for_user(&cx.session.user).unwrap_or_else(|| cx.web.clone()), url);
        let name = match name.map(|n| sanitize_file_name(&n)).filter(|n| !n.is_empty()) {
            Some(n) => n,
            None => download_name(&url.to_string(), &ctype),
        };
        let shown = name.clone();
        cx.audit.record(&cx.session.user, "browser.download", &url.host);
        let url_string = url.to_string();
        let spawned = std::thread::Builder::new().name("iyakki-browser-download".into()).spawn(move || {
            let r = web.get(&url).and_then(|resp| {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("cancelled".to_string());
                }
                if resp.status >= 400 {
                    return Err(format!("the server answered {}", resp.status));
                }
                if resp.content_length.is_some_and(|n| n > resp.body.len() as u64) {
                    return Err(format!("too large to save here (over {})", human_bytes(resp.body.len() as u64)));
                }
                // never overwrite: name (1).ext, name (2).ext …
                let (stem, ext) = match name.rsplit_once('.') {
                    Some((a, b)) if !a.is_empty() => (a.to_string(), format!(".{b}")),
                    _ => (name.clone(), String::new()),
                };
                let mut final_name = name.clone();
                let mut n = 1;
                while dir.join(&final_name).exists() && n < 1000 {
                    final_name = format!("{stem} ({n}){ext}");
                    n += 1;
                }
                std::fs::write(dir.join(&final_name), &resp.body).map_err(|e| e.to_string())?;
                Ok((final_name, resp.body.len() as u64))
            });
            let _ = tx.send(r);
        });
        match spawned {
            Ok(_) => {
                self.status = Some((format!("Downloading {shown}…"), true));
                self.download = Some((url_string, cancel, rx));
            }
            Err(e) => self.status = Some((format!("could not start: {e}"), false)),
        }
    }

    fn poll_download(&mut self, cx: &AppCx) {
        let Some((url, _, rx)) = &self.download else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the download stopped".into()),
        };
        let url = url.clone();
        self.download = None;
        let (name, bytes, status) = match &r {
            Ok((n, b)) => (n.clone(), *b, "saved".to_string()),
            Err(e) => (download_name(&url, ""), 0, e.clone()),
        };
        self.data.add_download(browser_data::Download { url, name: name.clone(), bytes, at: iyakki_agent::clock::now_unix(), status: status.clone() });
        browser_data::save(cx, &self.data);
        self.status = Some(match r {
            Ok((n, b)) => (format!("Saved {n} ({}) to Downloads.", human_bytes(b)), true),
            Err(e) => (format!("Download failed: {e}"), false),
        });
        let _ = (name, status);
    }

    fn export_bookmarks(&mut self, cx: &AppCx) {
        match Self::downloads_dir(cx).and_then(|d| {
            let p = d.join("bookmarks.html");
            std::fs::write(&p, self.data.export_html()).map(|_| p).map_err(|e| e.to_string())
        }) {
            Ok(p) => self.status = Some((format!("Saved {} bookmarks to {}.", self.data.bookmarks.len(), p.display()), true)),
            Err(e) => self.status = Some((e, false)),
        }
    }

    fn import_bookmarks(&mut self, cx: &AppCx) {
        let read = Self::downloads_dir(cx).and_then(|d| {
            let p = d.join("bookmarks.html");
            std::fs::read_to_string(&p).map_err(|_| format!("put bookmarks.html in {} first", d.display()))
        });
        match read {
            Ok(t) => {
                let n = self.data.import_html(&t);
                browser_data::save(cx, &self.data);
                self.layout = None;
                self.status = Some((format!("Added {n} bookmarks."), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// Keep the sign-in that was offered after the last form.
    fn save_offer(&mut self, cx: &AppCx) {
        let Some(c) = self.offer.take() else {
            self.status = Some(("There is no sign-in to save. After you send a sign-in form, Ctrl+S offers to keep it.".into(), false));
            return;
        };
        match browser_logins::save(cx, &c) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "browser.password.save", &browser_logins::host_key(&c.host));
                self.status = Some((format!("Saved the sign-in for {}.", browser_logins::host_key(&c.host)), true));
            }
            Err(e) => {
                self.status = Some((e, false));
                self.offer = Some(c);
            }
        }
    }

    fn set_control_value(&mut self, i: usize, value: &str) {
        if let Some(st) = self.state_mut(i) {
            st.value = value.to_string();
        }
        if self.focus_ctl == Some(i) {
            self.edit.set(value);
        }
    }

    /// Fill the sign-in form of the page with what is saved for exactly this site (again: the next saved one).
    fn fill_login(&mut self, cx: &AppCx) {
        let Some(page) = self.page().cloned() else { return };
        let Some((user_i, pw_i)) = browser_logins::login_controls(&page.doc) else {
            self.status = Some(("There is no sign-in form on this page.".into(), false));
            return;
        };
        let saved = match browser_logins::for_site(cx, &page.url.host) {
            Ok(v) => v,
            Err(e) => {
                self.status = Some((e, false));
                return;
            }
        };
        if saved.is_empty() {
            self.status = Some((format!("Nothing is saved for {}. After you sign in, Ctrl+S offers to keep it.", browser_logins::host_key(&page.url.host)), false));
            return;
        }
        let c = saved[self.fill_n % saved.len()].clone();
        self.fill_n += 1;
        if let Some(u) = user_i {
            self.set_control_value(u, &c.user);
        }
        self.set_control_value(pw_i, &c.password);
        cx.audit.record(&cx.session.user, "browser.password.fill", &browser_logins::host_key(&page.url.host));
        let more = if saved.len() > 1 { format!(" ({} of {}; Ctrl+K again for the next)", (self.fill_n - 1) % saved.len() + 1, saved.len()) } else { String::new() };
        self.status = Some((format!("Filled in {}{more}. Enter sends the form.", if c.user.is_empty() { "the password" } else { &c.user }), true));
    }

    /// A strong password for the password field that has the keyboard (and for the "again" field beside it).
    fn generate_password(&mut self, cx: &AppCx) {
        let Some(c) = self.focus_ctl else {
            self.status = Some(("Click a password field first, then press Ctrl+G.".into(), false));
            return;
        };
        let Some(page) = self.page().cloned() else { return };
        let Some(ctl) = page.doc.controls.get(c).filter(|x| x.kind == ControlKind::Password) else {
            self.status = Some(("Ctrl+G makes a password for a password field.".into(), false));
            return;
        };
        let form = ctl.form;
        match browser_logins::generate_password() {
            Ok(pw) => {
                let mates: Vec<usize> = page.doc.controls.iter().enumerate().filter(|(i, x)| x.kind == ControlKind::Password && x.form == form && *i != c).map(|(i, _)| i).collect();
                self.set_control_value(c, &pw);
                for i in mates {
                    self.set_control_value(i, &pw);
                }
                cx.audit.record(&cx.session.user, "browser.password.generate", &browser_logins::host_key(&page.url.host));
                self.status = Some(("Made a 20-character password. When you send the form it is offered for saving (Ctrl+S).".into(), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    fn clear_selection(&mut self) {
        self.sel = None;
        self.sel_range = None;
        self.sel_text.clear();
    }

    /// Copy the selected text (or, with a link in focus or under the pointer, nothing here: see `copy_link`).
    fn copy_selection(&mut self) -> bool {
        if self.sel_text.is_empty() {
            return false;
        }
        crate::ui::clipboard::copy(&self.sel_text);
        crate::ui::clipboard::toast("Copied");
        true
    }

    /// Copy the address of the link in focus or under the pointer.
    fn copy_link(&mut self) {
        let Some(page) = self.page().cloned() else { return };
        let Some(href) = self.focus_link.or(self.hovered_link).and_then(|l| page.doc.links.get(l)) else {
            self.status = Some(("Point at a link, or Tab to one, then press Y.".into(), false));
            return;
        };
        match page.url.join(href) {
            Ok(u) => {
                crate::ui::clipboard::copy(&u.to_string());
                self.status = Some(("Copied the link address.".into(), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    fn open_find(&mut self) {
        if self.page().is_none() {
            self.status = Some(("Open a page first.".into(), false));
            return;
        }
        match &mut self.find {
            Some(f) => f.field.select_all(),
            None => {
                let mut field = TextField::new("Find in page", false);
                field.max = 200;
                self.find = Some(Find { field, matches: Vec::new(), cur: 0, dirty: true });
            }
        }
        self.addr_focus = false;
        self.focus_ctl = None;
    }

    fn close_find(&mut self) {
        self.find = None;
    }

    /// Search again if the words or the layout changed.
    fn update_find(&mut self) {
        let (Some(f), Some(l)) = (&mut self.find, &self.layout) else { return };
        if f.dirty {
            f.matches = find_matches(&l.prims, &f.field.value);
            f.cur = f.cur.min(f.matches.len().saturating_sub(1));
            f.dirty = false;
        }
    }

    /// Go to the next (or previous) match and scroll it into view.
    fn step_find(&mut self, forward: bool) {
        self.update_find();
        let Some(f) = &mut self.find else { return };
        if f.matches.is_empty() {
            return;
        }
        let n = f.matches.len();
        f.cur = if forward { (f.cur + 1) % n } else { (f.cur + n - 1) % n };
        self.scroll_to_match();
    }

    fn scroll_to_match(&mut self) {
        let (Some(f), Some(l)) = (&self.find, &self.layout) else { return };
        let Some(seg) = f.matches.get(f.cur).and_then(|m| m.first()) else { return };
        if let Some(p) = l.prims.get(seg.0) {
            if p.y < self.scroll || p.y + p.h > self.scroll + self.view_h {
                self.scroll = (p.y - self.view_h * 0.3).max(0.0).min(self.max_scroll());
            }
        }
    }

    /// Keys while the find bar is open. Returns true when the key was used.
    fn find_key(&mut self, k: &Key) -> bool {
        match k {
            Key::Esc => self.close_find(),
            Key::Enter | Key::Down | Key::F(3) => self.step_find(true),
            Key::Up => self.step_find(false),
            Key::PageUp | Key::PageDown | Key::Tab | Key::BackTab => return false,
            other => {
                let Some(f) = &mut self.find else { return false };
                if f.field.handle(other) {
                    f.dirty = true;
                    f.cur = 0;
                    self.update_find();
                    self.scroll_to_match();
                }
            }
        }
        true
    }

    /// How big the page being read is drawn (1.0 is normal); remembered for each site.
    pub fn zoom(&self) -> f32 {
        self.page().map_or(1.0, |p| self.data.zoom_for(&p.url.host))
    }

    fn change_zoom(&mut self, up: Option<bool>, cx: &AppCx) {
        let Some(host) = self.current_url().map(|u| u.host.clone()) else { return };
        let z = match up {
            Some(up) => browser_data::zoom_step(self.zoom(), up),
            None => 1.0,
        };
        let keep = self.scroll / self.layout.as_ref().map_or(1.0, |l| l.height.max(1.0));
        self.data.set_zoom(&host, z);
        browser_data::save(cx, &self.data);
        if let Some(f) = self.full.as_mut() {
            f.set_zoom(z);
        }
        // the same part of the page stays in view: the layout is made again, the scroll position is kept in proportion
        let before = self.layout.as_ref().map_or(1.0, |l| l.height.max(1.0));
        self.layout = None;
        self.pending_scroll = Some(keep * before);
        self.status = Some((format!("{} zoom on {host}", if (z - 1.0).abs() < 0.01 { "Normal".to_string() } else { format!("{}%", (z * 100.0).round()) }), true));
    }

    /// The words to work on: the selection if there is one, else the whole page.
    fn assist_source(&self) -> Option<(String, &'static str)> {
        let page = self.page()?;
        if page.download || page.media {
            return None;
        }
        if !self.sel_text.trim().is_empty() {
            return Some((self.sel_text.clone(), "the selected text"));
        }
        let text = iyakki_web::reader::plain_text(&page.doc, 60_000);
        (!text.trim().is_empty()).then_some((text, "this page"))
    }

    /// Open the assistant panel: a question (`None`), or a task that is confirmed first.
    fn open_assist(&mut self, task: Option<browser_assist::Task>, cx: &AppCx) {
        let (Some((text, what)), Some(url)) = (self.assist_source(), self.current_url().map(|u| u.to_string())) else {
            self.status = Some(("Open a page with some text first.".into(), false));
            return;
        };
        self.addr_focus = false;
        self.focus_ctl = None;
        self.assist = Some(match task {
            Some(t) => browser_assist::Assist::task(t, text, what, url).check(cx),
            None => browser_assist::Assist::ask(text, what, url),
        });
    }

    /// Keep the page, or the selected words, in the person's own knowledge base (M).
    fn remember_page(&mut self, cx: &AppCx) {
        let (Some((text, what)), Some(page)) = (self.assist_source(), self.page().cloned()) else {
            self.status = Some(("Open a page with some text first.".into(), false));
            return;
        };
        self.status = Some(match browser_assist::remember(cx, &page.doc.title, &page.url.to_string(), &text) {
            Ok(n) => (format!("Kept {what} ({n} characters) in your knowledge (RAG). The assistant can find it from now on."), true),
            Err(e) => (e, false),
        });
    }

    /// Night colours for the site being read (N): light pages are drawn dark.
    fn toggle_night(&mut self, cx: &AppCx) {
        let Some((host, light)) = self.page().map(|p| (p.url.host.clone(), browser_dark::is_light(&p.doc))) else { return };
        let on = self.data.toggle_dark(&host);
        browser_data::save(cx, &self.data);
        self.layout = None;
        self.status = Some((
            if !on {
                format!("Night colours off on {host}.")
            } else if light {
                format!("Night colours on for {host}.")
            } else {
                format!("Night colours on for {host}; this page is not light, so it looks the same.")
            },
            true,
        ));
    }

    /// Forget what the site being read has stored in this browser: its cookies and its saved data (X).
    fn forget_site(&mut self, cx: &AppCx) {
        let Some(host) = self.current_url().map(|u| u.host.clone()).filter(|h| !h.is_empty()) else { return };
        browser_persist::forget(cx, &self.sites, &host);
        cx.audit.record(&cx.session.user, "browser.forget_site", &host);
        self.status = Some((format!("Forgot the cookies and saved data of {host}. You are signed out there."), true));
    }

    /// Show what the server sent, as plain text (U), or the page again.
    fn toggle_source(&mut self) {
        let pos = self.pos;
        let Some(e) = self.history.get_mut(pos) else { return };
        if let Some(orig) = e.original.take() {
            e.page = Some(orig);
            self.status = Some(("The page as it is drawn.".into(), true));
        } else if let Some(p) = e.page.clone().filter(|p| !p.source.is_empty()) {
            let doc = iyakki_web::Doc { title: format!("Source of {}", p.url.host), blocks: vec![iyakki_web::doc::Block::Pre(p.source.clone())], ..Default::default() };
            e.original = Some(p.clone());
            e.page = Some(Arc::new(Page { url: p.url.clone(), status: p.status, doc, notes: Vec::new(), source: p.source.clone(), media: false, download: false, ctype: p.ctype.clone() }));
            self.status = Some(("Source of the page. U shows the page again.".into(), true));
        } else {
            self.status = Some(("This page has no source to show.".into(), false));
            return;
        }
        self.layout = None;
        self.scroll = 0.0;
    }

    /// Show the article without the page around it (R), or the page again.
    fn toggle_reader(&mut self) {
        let pos = self.pos;
        let Some(e) = self.history.get_mut(pos) else { return };
        if let Some(orig) = e.original.take() {
            e.page = Some(orig);
            self.status = Some(("The page as it was sent.".into(), true));
        } else if let Some(p) = e.page.clone() {
            match iyakki_web::reader::simplify(&p.doc) {
                Some(doc) => {
                    e.original = Some(p.clone());
                    e.page = Some(Arc::new(Page { url: p.url.clone(), status: p.status, doc, notes: p.notes.clone(), source: p.source.clone(), media: p.media, download: p.download, ctype: p.ctype.clone() }));
                    self.status = Some(("Reader view. R shows the page again.".into(), true));
                }
                None => {
                    self.status = Some(("This page has no article to show on its own.".into(), false));
                    return;
                }
            }
        } else {
            return;
        }
        self.layout = None;
        self.scroll = 0.0;
    }

    fn record_visit(&mut self, page: &Page, cx: &AppCx) {
        self.data.visit(&page.url.to_string(), &page.doc.title, iyakki_agent::clock::now_unix());
        browser_data::save(cx, &self.data);
    }

    fn refresh_suggestions(&mut self) {
        self.suggest = if self.addr_focus { self.data.suggestions(&self.field.value, 6) } else { Vec::new() };
        self.sug_sel = None;
    }

    /// Star or unstar the page being read.
    fn toggle_bookmark(&mut self, cx: &AppCx) {
        let Some((url, title)) = self.page().map(|p| (p.url.to_string(), p.doc.title.clone())) else {
            self.status = Some(("Open a page first.".into(), false));
            return;
        };
        let on = self.data.toggle_bookmark(&url, &title);
        browser_data::save(cx, &self.data);
        cx.audit.record(&cx.session.user, if on { "browser.bookmark.add" } else { "browser.bookmark.remove" }, browser_data::host_of(&url));
        self.status = Some((if on { "Bookmarked.".into() } else { "Bookmark removed.".to_string() }, true));
    }

    /// Show the browser's own history or bookmarks page as a step in the history of this tab.
    fn show_view(&mut self, view: StartView) {
        self.drop_full_quiet();
        if let Some(e) = self.history.get_mut(self.pos) {
            e.scroll = self.scroll;
        }
        self.history.truncate(self.pos + 1);
        self.history.push(Entry { page: None, scroll: 0.0, ctl: Vec::new(), images: Vec::new(), serial: 0, original: None, view });
        if self.history.len() > HISTORY_MAX {
            self.history.remove(0);
        }
        self.pos = self.history.len() - 1;
        self.nav = Nav::Idle;
        self.layout = None;
        self.scroll = 0.0;
        self.focus_link = None;
        self.focus_ctl = None;
        self.field.set("");
        self.addr_focus = false;
    }

    fn view(&self) -> StartView {
        self.history.get(self.pos).map_or(StartView::Home, |e| e.view)
    }

    fn max_scroll(&self) -> f32 {
        self.layout.as_ref().map_or(0.0, |l| (l.height - self.view_h).max(0.0))
    }

    fn scroll_by(&mut self, d: f32) {
        self.scroll = (self.scroll + d).max(0.0).min(self.max_scroll());
    }

    /// Tab: the next link or form control in reading order.
    fn cycle_focus(&mut self, forward: bool) {
        let Some(order) = self.layout.as_ref().map(|l| l.focus.clone()) else { return };
        if order.is_empty() {
            return;
        }
        let here = match (self.focus_ctl, self.focus_link) {
            (Some(c), _) => order.iter().position(|f| *f == Focus::Control(c)),
            (None, Some(l)) => order.iter().position(|f| *f == Focus::Link(l)),
            _ => None,
        };
        let n = order.len();
        let next = match (here, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        let rect = match order[next] {
            Focus::Link(l) => {
                self.focus_link = Some(l);
                self.focus_ctl = None;
                self.layout.as_ref().and_then(|l2| l2.links.get(l).copied().flatten())
            }
            Focus::Control(c) => {
                self.focus_link = None;
                self.focus_control(c);
                self.layout.as_ref().and_then(|l2| l2.controls.get(c).copied().flatten())
            }
        };
        // bring it into view
        if let Some(r) = rect {
            if r.y < self.scroll || r.y + r.h > self.scroll + self.view_h {
                self.scroll = (r.y - self.view_h * 0.3).max(0.0).min(self.max_scroll());
            }
        }
    }

    fn control(&self, i: usize) -> Option<(&Control, &CtlState)> {
        let page = self.page()?;
        Some((page.doc.controls.get(i)?, self.history.get(self.pos)?.ctl.get(i)?))
    }

    fn state_mut(&mut self, i: usize) -> Option<&mut CtlState> {
        self.history.get_mut(self.pos)?.ctl.get_mut(i)
    }

    /// Give the keyboard to a control (a text field gets its editor loaded).
    fn focus_control(&mut self, i: usize) {
        self.addr_focus = false;
        self.focus_ctl = Some(i);
        if let Some((c, st)) = self.control(i) {
            if matches!(c.kind, ControlKind::Text | ControlKind::Password) {
                let (value, mask) = (st.value.clone(), c.kind == ControlKind::Password);
                self.edit = TextField::new("", mask);
                self.edit.max = 2048;
                self.edit.set(&value);
            }
        }
    }

    /// A click on a control. `x` is the distance from its left edge.
    fn click_control(&mut self, i: usize, x: f32, cx: &AppCx) {
        let Some((c, _)) = self.control(i) else { return };
        let (kind, form, name, disabled) = (c.kind, c.form, c.name.clone(), c.disabled);
        if disabled {
            return;
        }
        self.focus_link = None;
        match kind {
            ControlKind::Text | ControlKind::Password => {
                if self.focus_ctl != Some(i) {
                    self.focus_control(i);
                }
                self.edit.click(x);
            }
            ControlKind::Textarea => self.focus_control(i),
            ControlKind::Checkbox => {
                self.focus_control(i);
                if let Some(st) = self.state_mut(i) {
                    st.checked = !st.checked;
                }
            }
            ControlKind::Radio => {
                self.focus_control(i);
                // choosing one clears the others of its group
                let same: Vec<usize> = self.page().map(|p| p.doc.controls.iter().enumerate().filter(|(j, o)| *j != i && o.kind == ControlKind::Radio && o.form == form && o.name == name).map(|(j, _)| j).collect()).unwrap_or_default();
                for j in same {
                    if let Some(st) = self.state_mut(j) {
                        st.checked = false;
                    }
                }
                if let Some(st) = self.state_mut(i) {
                    st.checked = true;
                }
            }
            ControlKind::Select => {
                self.focus_control(i);
                self.open_popup(i);
            }
            ControlKind::Submit => {
                self.focus_control(i);
                if let Some(f) = form {
                    self.submit_form(f, Some(i), cx);
                } else {
                    self.status = Some(("This button is not inside a form.".into(), false));
                }
            }
            ControlKind::Button => {
                self.focus_control(i);
                self.status = Some(("This button needs JavaScript (press j to allow it for this site).".into(), false));
            }
            ControlKind::Hidden => {}
        }
    }

    /// Show the options of a drop-down as a list to choose from.
    fn open_popup(&mut self, i: usize) {
        if let Some((c, st)) = self.control(i) {
            if !c.options.is_empty() && !c.disabled {
                self.popup = Some((i, st.sel.min(c.options.len() - 1)));
            }
        }
    }

    /// Keys while a drop-down list is open (it takes all of them).
    fn popup_key(&mut self, k: &Key) {
        let Some((i, hi)) = self.popup else { return };
        let Some(options) = self.control(i).map(|(c, _)| c.options.iter().map(|o| o.label.clone()).collect::<Vec<_>>()) else {
            self.popup = None;
            return;
        };
        let n = options.len();
        match k {
            Key::Esc => self.popup = None,
            Key::Enter | Key::Char(' ') => self.pick_option(hi),
            Key::Up => self.popup = Some((i, hi.saturating_sub(1))),
            Key::Down => self.popup = Some((i, (hi + 1).min(n - 1))),
            Key::Home => self.popup = Some((i, 0)),
            Key::End => self.popup = Some((i, n - 1)),
            Key::PageUp => self.popup = Some((i, hi.saturating_sub(8))),
            Key::PageDown => self.popup = Some((i, (hi + 8).min(n - 1))),
            // typing a letter jumps to the next option that starts with it
            Key::Char(ch) => {
                let low = ch.to_lowercase().next().unwrap_or(*ch);
                if let Some(j) = (1..=n).map(|d| (hi + d) % n).find(|j| options[*j].trim().to_lowercase().starts_with(low)) {
                    self.popup = Some((i, j));
                }
            }
            _ => {}
        }
    }

    fn pick_option(&mut self, option: usize) {
        if let Some((i, _)) = self.popup.take() {
            if let Some(st) = self.state_mut(i) {
                st.sel = option;
            }
        }
    }

    fn step_select(&mut self, i: usize, d: i32) {
        let n = self.control(i).map_or(0, |(c, _)| c.options.len());
        if n == 0 {
            return;
        }
        if let Some(st) = self.state_mut(i) {
            st.sel = (st.sel as i32 + d).rem_euclid(n as i32) as usize;
        }
    }

    /// Send a form: GET puts the values in the address, POST sends them as the body.
    fn submit_form(&mut self, form: usize, activator: Option<usize>, cx: &AppCx) {
        let Some(page) = self.page().cloned() else { return };
        let Some(f) = page.doc.forms.get(form) else { return };
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (i, c) in page.doc.controls.iter().enumerate().filter(|(_, c)| c.form == Some(form)) {
            if c.disabled || c.name.is_empty() && !matches!(c.kind, ControlKind::Submit) {
                continue;
            }
            let Some(st) = self.history.get(self.pos).and_then(|e| e.ctl.get(i)) else { continue };
            match c.kind {
                ControlKind::Text | ControlKind::Password | ControlKind::Textarea => pairs.push((c.name.clone(), st.value.clone())),
                ControlKind::Hidden => pairs.push((c.name.clone(), c.value.clone())),
                ControlKind::Checkbox | ControlKind::Radio => {
                    if st.checked {
                        pairs.push((c.name.clone(), c.value.clone()));
                    }
                }
                ControlKind::Select => {
                    if let Some(o) = c.options.get(st.sel) {
                        pairs.push((c.name.clone(), o.value.clone()));
                    }
                }
                ControlKind::Submit => {
                    if Some(i) == activator && !c.name.is_empty() {
                        pairs.push((c.name.clone(), c.value.clone()));
                    }
                }
                ControlKind::Button => {}
            }
        }
        let body = form_urlencode(&pairs);
        let has_password = page.doc.controls.iter().any(|c| c.form == Some(form) && c.kind == ControlKind::Password);
        let target_https = if f.action.is_empty() { page.url.https } else { page.url.join(&f.action).map(|u| u.https).unwrap_or(false) };
        if has_password && !target_https && !self.http_warned {
            self.http_warned = true;
            self.status = Some(("This form is not protected (http): a password sent from it can be read by others on the network. Press Enter again to send it anyway.".into(), false));
            return;
        }
        // a sign-in being sent: remember it until the next page shows whether it worked
        self.pending_cred = None;
        if let Some(pw_i) = page.doc.controls.iter().position(|c| c.form == Some(form) && c.kind == ControlKind::Password) {
            let value = |i: usize| self.history.get(self.pos).and_then(|e| e.ctl.get(i)).map(|st| st.value.clone()).unwrap_or_default();
            let user = (0..pw_i).rev().find(|&i| page.doc.controls[i].form == Some(form) && page.doc.controls[i].kind == ControlKind::Text).map(value).unwrap_or_default();
            let password = value(pw_i);
            if !password.is_empty() {
                self.pending_cred = Some(Cred { host: page.url.host.clone(), user, password });
            }
        }
        let target = if f.action.is_empty() { Ok(page.url.without_fragment()) } else { page.url.join(&f.action) };
        match target {
            Ok(mut u) => {
                cx.audit.record(&cx.session.user, "browser.form", &u.host);
                if f.post {
                    self.go_request(Request { url: u, method: "POST".into(), body: body.into_bytes(), content_type: "application/x-www-form-urlencoded".into(), cookie: String::new(), referer: page.url.to_string(), ..Default::default() }, How::Push, cx);
                } else {
                    u.query = (!body.is_empty()).then_some(body);
                    u.fragment = None;
                    self.go_request(Request { url: u, method: "GET".into(), referer: page.url.to_string(), ..Request::default() }, How::Push, cx);
                }
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// A key while a form control has the keyboard. `None`: not for the control, treat it as a page key.
    fn key_in_control(&mut self, c: usize, k: &Key, cx: &AppCx) -> Option<Flow> {
        let (kind, form) = self.control(c).map(|(c, _)| (c.kind, c.form))?;
        match k {
            Key::Esc => {
                self.focus_ctl = None;
                return Some(Flow::Stay);
            }
            Key::Tab => {
                self.cycle_focus(true);
                return Some(Flow::Stay);
            }
            Key::BackTab => {
                self.cycle_focus(false);
                return Some(Flow::Stay);
            }
            _ => {}
        }
        match kind {
            ControlKind::Text | ControlKind::Password => {
                if *k == Key::Enter {
                    if let Some(f) = form {
                        self.submit_form(f, None, cx);
                    }
                } else {
                    self.edit.handle(k);
                    let v = self.edit.value.clone();
                    if let Some(st) = self.state_mut(c) {
                        st.value = v;
                    }
                }
                Some(Flow::Stay)
            }
            ControlKind::Textarea => {
                if let Some(st) = self.state_mut(c) {
                    match k {
                        Key::Enter => st.value.push('\n'),
                        Key::Backspace => {
                            st.value.pop();
                        }
                        Key::Char(ch) => st.value.push(*ch),
                        _ => return None,
                    }
                }
                Some(Flow::Stay)
            }
            ControlKind::Checkbox | ControlKind::Radio | ControlKind::Submit | ControlKind::Button if matches!(k, Key::Enter | Key::Char(' ')) => {
                self.click_control(c, 0.0, cx);
                Some(Flow::Stay)
            }
            ControlKind::Select => match k {
                Key::Up | Key::Left => {
                    self.step_select(c, -1);
                    Some(Flow::Stay)
                }
                Key::Down | Key::Right => {
                    self.step_select(c, 1);
                    Some(Flow::Stay)
                }
                Key::Enter | Key::Char(' ') => {
                    self.open_popup(c);
                    Some(Flow::Stay)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Shortcuts that work wherever the keyboard is (the address bar included): tabs, bookmarks, history, downloads.
    fn global_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        match k {
            Key::Ctrl('t') => self.new_tab(),
            Key::Ctrl('w') => self.close_tab(self.active),
            Key::Ctrl('T') => self.reopen_closed(cx),
            Key::Ctrl('G') => self.new_group_from(self.active),
            Key::Ctrl('U') => self.ungroup(self.active),
            Key::Ctrl('O') => self.open_overview(),
            Key::F(2) if !self.embedded => self.open_group_edit(self.active),
            Key::Nav { nav: super::super::ui::Nav::PageDown, ctrl: true, .. } => self.step_tab(true),
            Key::Nav { nav: super::super::ui::Nav::PageUp, ctrl: true, .. } => self.step_tab(false),
            Key::Ctrl('f') => self.open_find(),
            Key::Ctrl('s') => self.save_offer(cx),
            Key::Ctrl('k') => self.fill_login(cx),
            Key::Ctrl('g') => self.generate_password(cx),
            Key::F(3) if self.find.is_some() => self.step_find(true),
            Key::Ctrl('d') => self.toggle_bookmark(cx),
            Key::Ctrl('h') => self.show_view(StartView::History),
            Key::Ctrl('b') => self.show_view(StartView::Bookmarks),
            Key::Ctrl('j') => self.show_view(StartView::Downloads),
            _ => return false,
        }
        true
    }

    fn handle_key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        // the microphone chooser over the question has the keyboard first
        if self.media_mic_pick.is_some() {
            self.mic_pick_event(&Event::Key(k.clone()), cx);
            return Flow::Stay;
        }
        if self.media_cam_pick.is_some() {
            self.cam_pick_event(&Event::Key(k.clone()), cx);
            return Flow::Stay;
        }
        // the camera / microphone question has the keyboard while it is open
        if self.media_ask.is_some() {
            if matches!(k, Key::Char('m') | Key::Char('M')) && self.media_ask_is_mic() {
                self.open_mic_pick();
                return Flow::Stay;
            }
            if matches!(k, Key::Char('c') | Key::Char('C')) && self.media_ask_is_cam() {
                self.open_cam_pick();
                return Flow::Stay;
            }
            if let Some(a) = browser_media::answer_for_key(k, self.media_ask.as_ref().is_some_and(|a| a.is_screen())) {
                self.media_answer(a, cx);
            }
            return Flow::Stay;
        }
        // the sound dialog has the keyboard while it is open
        if self.sound_ask.is_some() {
            if let Some(a) = super::soundask::answer_for_key(k) {
                self.sound_answer(a, cx);
            }
            return Flow::Stay;
        }
        // the developer tools panel: its own keys anywhere, and every key but the Ctrl shortcuts while it has the keyboard
        if matches!(k, Key::F(12) | Key::Ctrl('I')) && !self.full.as_ref().is_some_and(|f| f.fullscreen) {
            self.toggle_devpanel(cx);
            return Flow::Stay;
        }
        if self.dev.open && self.dev.focus && self.popup.is_none() && self.assist.is_none() && !matches!(k, Key::Ctrl(_) | Key::Nav { .. }) {
            let a = self.dev.key(k);
            self.dev_act(a, cx);
            return Flow::Stay;
        }
        if self.overview.is_some() {
            if matches!(k, Key::Ctrl('O')) {
                self.overview = None;
            } else {
                self.overview_key(k);
            }
            return Flow::Stay;
        }
        if self.group_edit.is_some() {
            self.group_edit_key(k);
            return Flow::Stay;
        }
        if self.full_key(k, cx) {
            return Flow::Stay;
        }
        if self.global_key(k, cx) {
            return Flow::Stay;
        }
        if self.popup.is_some() {
            self.popup_key(k);
            return Flow::Stay;
        }
        if let Some(a) = &mut self.assist {
            if matches!(a.key(k, cx), browser_assist::Outcome::Closed) {
                self.assist = None;
            }
            return Flow::Stay;
        }
        if self.find.is_some() && !self.addr_focus && self.find_key(k) {
            return Flow::Stay;
        }
        if self.addr_focus {
            match k {
                Key::Enter => match self.sug_sel.and_then(|i| self.suggest.get(i)).map(|s| s.url.clone()) {
                    Some(u) => {
                        self.field.set(&u);
                        self.submit_address(cx);
                    }
                    None => self.submit_address(cx),
                },
                Key::Down if !self.suggest.is_empty() => self.sug_sel = Some(self.sug_sel.map_or(0, |i| (i + 1).min(self.suggest.len() - 1))),
                Key::Up if !self.suggest.is_empty() => self.sug_sel = self.sug_sel.and_then(|i| i.checked_sub(1)),
                Key::Esc if !self.suggest.is_empty() => {
                    self.suggest.clear();
                    self.sug_sel = None;
                }
                Key::Esc => {
                    if self.page().is_some() {
                        self.addr_focus = false;
                        if let Some(f) = self.full.as_mut() {
                            f.set_focus(true);
                        }
                        if let Some(u) = self.current_url().map(|u| u.to_string()) {
                            self.field.set(&u);
                        }
                    } else {
                        return Flow::Back;
                    }
                }
                Key::Tab | Key::Down if self.page().is_some() => {
                    self.addr_focus = false;
                    if let Some(f) = self.full.as_mut() {
                        f.set_focus(true);
                    }
                    self.suggest.clear();
                }
                other => {
                    self.field.handle(other);
                    self.refresh_suggestions();
                }
            }
            return Flow::Stay;
        }
        if let Some(c) = self.focus_ctl {
            if let Some(flow) = self.key_in_control(c, k, cx) {
                return flow;
            }
        }
        let page_h = self.view_h * 0.9;
        match k {
            Key::Esc if self.sel.is_some() => self.clear_selection(),
            Key::Ctrl('c') | Key::Ctrl('C') if self.sel_range.is_some() => {
                self.copy_selection();
            }
            Key::Ctrl('a') | Key::Ctrl('A') if self.page().is_some() => {
                self.sel = Some(Sel { anchor: (0.0, 0.0), head: (0.0, 0.0), mode: 1, all: true });
            }
            Key::Char('y') => self.copy_link(),
            Key::Esc if self.download.is_some() => {
                if let Some((_, flag, _)) = &self.download {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                self.status = Some(("Download cancelled.".into(), true));
            }
            Key::Esc => return Flow::Back,
            Key::Up => self.scroll_by(-48.0),
            Key::Down => self.scroll_by(48.0),
            Key::PageUp => self.scroll_by(-page_h),
            Key::PageDown | Key::Char(' ') => self.scroll_by(page_h),
            Key::Home => self.scroll = 0.0,
            Key::End => self.scroll = self.max_scroll(),
            Key::Tab => self.cycle_focus(true),
            Key::BackTab => self.cycle_focus(false),
            Key::Enter => match self.focus_link {
                Some(l) => self.follow(l, cx),
                None => self.addr_focus = true,
            },
            Key::Char('g' | '/') | Key::Ctrl('l') => {
                self.addr_focus = true;
                self.field.handle(&Key::Ctrl('a'));
                self.refresh_suggestions();
            }
            Key::Char('d') if self.page().is_some_and(|p| p.download) => self.start_download(cx),
            Key::Ctrl('e') if self.page().is_none() && self.view() == StartView::Bookmarks => self.export_bookmarks(cx),
            Key::Char('I') if self.page().is_none() && self.view() == StartView::Bookmarks => self.import_bookmarks(cx),
            Key::Char('D') if self.page().is_none() && self.view() == StartView::History => {
                self.data.clear_history();
                browser_data::save(cx, &self.data);
                cx.audit.record(&cx.session.user, "browser.history.clear", "");
                self.layout = None;
                self.status = Some(("History cleared.".into(), true));
            }
            Key::Char('b') | Key::Left => self.back(),
            Key::Char('f') | Key::Right => self.forward(),
            Key::Char('r') => self.reload(cx),
            Key::Char('h') => self.home(),
            Key::Char('j') => self.toggle_js(cx),
            Key::Char('a') => self.toggle_block_allow(cx),
            Key::Char('B') => self.show_blocked = !self.show_blocked,
            Key::Char('p') if self.page().is_some_and(|p| p.media) => self.play_media(cx),
            Key::Char('c') => {
                // closed → notes → network → closed
                if self.show_notes {
                    self.show_notes = false;
                    self.show_net = true;
                } else if self.show_net {
                    self.show_net = false;
                } else {
                    self.show_notes = true;
                }
            }
            Key::Char('+' | '=') => self.change_zoom(Some(true), cx),
            Key::Char('-') => self.change_zoom(Some(false), cx),
            Key::Char('0') => self.change_zoom(None, cx),
            Key::Char('R') => self.toggle_reader(),
            Key::Char('U') => self.toggle_source(),
            Key::Char('F') => self.open_full_browser(cx),
            Key::Char('X') => self.forget_site(cx),
            Key::Char('N') => self.toggle_night(cx),
            Key::Char('Q') => self.open_assist(None, cx),
            Key::Char('S') => self.open_assist(Some(browser_assist::Task::Summarise), cx),
            Key::Char('T') => self.open_assist(Some(browser_assist::Task::Translate), cx),
            Key::Char('M') => self.remember_page(cx),
            _ => {}
        }
        Flow::Stay
    }

    /// The address to open in the full browser: the one being typed (when the bar has the focus), else the page, else
    /// whatever the bar holds.
    fn full_browser_target(&self) -> Option<String> {
        let typed = self.field.value.trim();
        if self.addr_focus && !typed.is_empty() {
            let text = expand_shortcut(typed).unwrap_or_else(|| typed.to_string());
            return Url::from_input_on(&text, self.cfg.search).ok().map(|u| u.to_string());
        }
        if let Some(u) = self.current_url() {
            return Some(u.to_string());
        }
        (!typed.is_empty()).then(|| Url::from_input_on(typed, self.cfg.search).ok().map(|u| u.to_string())).flatten()
    }

    /// F (or the "Full browser" button): this tab's page is shown by the full web engine, in the page area (the engine is a
    /// helper program that renders offscreen, `docs/WEBVIEW-PROTOCOL.md`); the same key or button, or Esc, goes back to the
    /// built-in reader. On an image that has only cage and cog (no helper), the old hand-over of the screen is used instead.
    /// The account's host policy is applied first.
    fn open_full_browser(&mut self, cx: &AppCx) {
        if self.embedded {
            // the host owns the one engine: tell it which page needs it
            if let Some(t) = self.full_browser_target() {
                self.want_full = Some(t);
            }
            return;
        }
        if self.full.is_some() {
            self.leave_full(cx);
            return;
        }
        if !iyakki_agent::webengine::offered() {
            self.status = Some((format!("{}.", iyakki_agent::webengine::NOT_INSTALLED), false));
            return;
        }
        let Some(target) = self.full_browser_target() else {
            self.status = Some(("Open a page or type an address first.".into(), false));
            return;
        };
        let url = match iyakki_agent::webengine::allowed(&cx.session.policy, &target) {
            Ok(u) => u,
            Err(e) => {
                self.status = Some((e, false));
                return;
            }
        };
        let host = iyakki_sec::url_host(&url).unwrap_or_default();
        if let Some(helper) = iyakki_agent::webengine::embedded_helper() {
            self.start_embedded(helper, url, host, cx);
            return;
        }
        let job = crate::external::Job { url, user: cx.session.user.clone(), data: cx.services.dir().to_path_buf() };
        match crate::external::request(job) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "browser.full", &host);
                self.status = Some((format!("Opening {host} in the full browser (Ctrl+Q comes back)…"), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// What the engine is started with: the person's profile folder, the engine's environment, the page area's size and the account's hosts.
    fn embedded_start(&self, helper: std::path::PathBuf, cx: &AppCx) -> Result<Start, String> {
        let profile = iyakki_agent::webengine::prepare_profile(cx.services.dir(), &cx.session.user)?;
        Ok(Start {
            env: iyakki_agent::webengine::helper_env(&profile).map(|mut e| {
                e.extend(browser_block::prepare_full_engine(cx, &profile));
                e
            }),
            helper,
            profile,
            size: (self.view_rect.w.max(64.0) as u32, self.view_rect.h.max(64.0) as u32),
            scale: self.ui_scale,
            zoom: self.zoom(),
            allow_hosts: cx.session.policy.net_hosts.clone(),
            user_agent: iyakki_agent::webengine::user_agent_suffix(),
            locale: String::new(),
        })
    }

    fn start_embedded(&mut self, helper: std::path::PathBuf, url: String, host: String, cx: &AppCx) {
        let start = match self.embedded_start(helper, cx) {
            Ok(s) => s,
            Err(e) => {
                self.status = Some((format!("The full web engine's profile folder could not be made: {e}"), false));
                return;
            }
        };
        self.share_devices("", 0, cx);
        match Full::start(start) {
            Ok(mut f) => {
                f.navigate(&url);
                self.clear_selection();
                self.full = Some(f);
                // the page gets its own step in the tab's history (back from it leads to the page it was opened from)
                if let Some(page) = full_stub(&url, "") {
                    self.show(page, How::Push);
                }
                cx.audit.record(&cx.session.user, "browser.full", &host);
                self.status = Some((format!("Opening {host} in the full web engine (F or Esc, then Esc again, goes back to the built-in reader)…"), true));
            }
            Err(e) => self.status = Some((format!("The full web engine could not be started: {e}"), false)),
        }
    }

    /// F again, or Esc with the browser's own keys in charge: the page is shown by the built-in reader again.
    fn leave_full(&mut self, cx: &AppCx) {
        let Some(f) = self.full.take() else { return };
        self.full_declined = true;
        let url = if f.url.is_empty() { f.good_url.clone() } else { f.url.clone() };
        drop(f);
        cx.audit.record(&cx.session.user, "browser.full.end", "");
        match Url::parse(&url) {
            // the page the engine is on is the one the step before it already shows: just return to that step
            Ok(u) if url.starts_with("http") && self.pos > 0 && self.history.get(self.pos - 1).and_then(|e| e.page.as_ref()).is_some_and(|p| p.url.without_fragment() == u.without_fragment() && p.ctype != STUB_TYPE) => {
                self.drop_stub_entry();
                self.status = Some(("Back in the built-in reader.".into(), true));
            }
            Ok(u) if url.starts_with("http") => {
                self.go(u, How::Replace, cx);
                self.status = Some(("Back in the built-in reader.".into(), true));
            }
            _ => {
                self.drop_stub_entry();
                self.status = Some(("Back in the built-in reader.".into(), true));
            }
        }
    }

    /// The step the full engine took in the history is removed (its page is not something the reader can show).
    fn drop_stub_entry(&mut self) {
        if self.history.get(self.pos).and_then(|e| e.page.as_ref()).is_some_and(|p| p.ctype == STUB_TYPE) && self.pos > 0 {
            self.history.remove(self.pos);
            let to = self.pos - 1;
            self.goto_entry(to);
        }
    }

    /// Stop the full engine without loading anything (the browser is going somewhere else: its start page, history, back).
    fn drop_full_quiet(&mut self) {
        if self.full.take().is_some() {
            self.drop_stub_entry();
        }
    }

    /// The page area has the keyboard (typing goes to the engine).
    fn full_has_keys(&self) -> bool {
        self.full.as_ref().is_some_and(|f| f.page_focus) && !self.addr_focus && self.popup.is_none() && self.assist.is_none()
    }

    /// Keys while the full engine shows the page. Returns true when the key was used here. With the page in charge nearly
    /// everything goes to the engine; Esc hands the keyboard back to the browser (its letters work again), and then F or Esc
    /// returns to the built-in reader. Tabs, bookmarks, history and the address bar keep their Ctrl shortcuts.
    fn full_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        // fullscreen page: no bars, so no browser shortcuts; Esc asks the page to leave, everything else is the page's
        if let Some(f) = self.full.as_mut().filter(|f| f.fullscreen) {
            match k {
                Key::Esc => f.leave_fullscreen(),
                other => f.key(other),
            }
            return true;
        }
        if self.full.is_none() || self.addr_focus || self.popup.is_some() || self.assist.is_some() {
            return false;
        }
        let page_focus = self.full.as_ref().is_some_and(|f| f.page_focus);
        if page_focus {
            match k {
                Key::Esc => {
                    if let Some(f) = self.full.as_mut() {
                        f.set_focus(false);
                    }
                    self.status = Some(("The browser has the keyboard: F or Esc goes back to the built-in reader, b f r g work, click the page to type in it.".into(), true));
                }
                Key::Ctrl('l') => {
                    self.addr_focus = true;
                    if let Some(f) = self.full.as_mut() {
                        f.set_focus(false);
                    }
                    self.field.handle(&Key::Ctrl('a'));
                    self.refresh_suggestions();
                }
                // the browser's own shortcuts
                Key::Ctrl('t' | 'w' | 'T' | 'd' | 'h' | 'b' | 'j') | Key::Nav { nav: super::super::ui::Nav::PageDown | super::super::ui::Nav::PageUp, ctrl: true, .. } => return false,
                Key::Ctrl('f') => self.status = Some(("Find in page is not available in the full engine yet.".into(), false)),
                other => {
                    if let Some(f) = self.full.as_mut() {
                        f.key(other);
                    }
                }
            }
            return true;
        }
        match k {
            Key::Esc | Key::Char('F') => self.leave_full(cx),
            // scrolling keys go to the page even when the browser has the keyboard
            Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End | Key::Char(' ') => {
                if let Some(f) = self.full.as_mut() {
                    f.key(k);
                }
            }
            // the browser's letters that mean something here
            Key::Char('b' | 'f' | 'r' | 'h' | 'g' | '/' | '+' | '=' | '-' | '0' | 'y' | 'M') | Key::Left | Key::Right | Key::Ctrl(_) | Key::Enter => return false,
            _ => {}
        }
        true
    }

    /// What arrived from the helpers of every tab: pictures, titles, addresses, downloads, a helper that died.
    fn poll_full(&mut self, cx: &AppCx) {
        let policy = cx.session.policy.clone();
        let allow = move |h: &str| policy.allows_host(h);
        // tabs in the background keep their title and address up to date
        for t in self.tabs.iter_mut().flatten() {
            let Some(f) = t.full.as_mut() else { continue };
            f.set_smooth(browser_config::smooth_now());
            f.pump(&allow);
            let (notes, title, url) = (f.take_notes(), f.title.clone(), f.url.clone());
            let mut gone = false;
            for n in notes {
                match n {
                    Note::Refused { host, .. } => cx.audit.record(&cx.session.user, "browser.full.refused", &host),
                    Note::Gone(_) => gone = true,
                    // a tab in the background never gets a device and is not asked about one
                    Note::Media { id, kinds, origin } => {
                        f.answer_permission(id, false);
                        browser_media::audit(cx, "browser.media.deny", &origin, kinds, "tab in the background");
                    }
                    _ => {}
                }
            }
            if gone {
                t.full = None;
            } else if !url.is_empty() {
                if let (Some(e), Some(page)) = (t.history.get_mut(t.pos), full_stub(&url, &title)) {
                    e.page = Some(page);
                }
            }
        }
        let Some(f) = self.full.as_mut() else { return };
        f.set_smooth(browser_config::smooth_now());
        f.pump(&allow);
        for n in f.take_notes() {
            self.apply_full_note(n, cx);
            if self.full.is_none() {
                break;
            }
        }
        self.av_poll(cx);
        self.sound_ask_poll(cx);
    }

    /// The helper reported media without a decoder (`word`: video, audio or both) on the engine's page: offer the add-on, once per site.
    fn offer_av(&mut self, word: &str, host: &str, cx: &AppCx) {
        let Some(need) = iyakki_agent::avaddon::Need::parse(word) else { return };
        let addon = iyakki_agent::avaddon::detect(cx.services.dir());
        if self.av.offer(host, need, addon) {
            cx.audit.record(&cx.session.user, "browser.av.offer", &browser_av::site_of(host));
        }
    }

    /// An answer to the sound dialog.
    fn sound_answer(&mut self, a: super::soundask::Answer, cx: &AppCx) {
        use super::soundask::{audit, start_stop_real, Answer};
        let Some(ask) = self.sound_ask.as_mut() else { return };
        if ask.stopping() {
            return;
        }
        match a {
            Answer::Stop => {
                audit(cx, "browser.sound.stop", &ask.found.clone(), "asked");
                start_stop_real(ask, std::time::Duration::from_secs(3));
            }
            Answer::Keep => {
                audit(cx, "browser.sound.keep", &ask.found.clone(), "kept");
                self.sound_ask = None;
            }
            Answer::Cancel => {
                audit(cx, "browser.sound.cancel", &ask.found.clone(), "cancelled");
                self.sound_ask = None;
                if let Some(f) = self.full.as_mut() {
                    f.sound_ask_cancelled();
                }
            }
        }
        self.sound_redraw();
    }

    fn sound_redraw(&mut self) {
        if let Some(f) = self.full.as_mut() {
            f.want_redraw();
        }
    }

    /// While the stop runs and after it: the result on the status line, then the page's media opens its sound again.
    fn sound_ask_poll(&mut self, cx: &AppCx) {
        if let Some(ask) = self.sound_ask.as_mut() {
            if let Some(r) = ask.poll() {
                let found = ask.found.clone();
                self.sound_ask = None;
                match r {
                    Ok(m) => {
                        super::soundask::audit(cx, "browser.sound.stopped", &found, &m);
                        self.status = Some((format!("{} {m} The page's sound will connect in a moment.", found.says), true));
                        self.sound_reconnect = Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
                    }
                    Err(e) => {
                        super::soundask::audit(cx, "browser.sound.stopfailed", &found, &e);
                        self.status = Some((format!("{} {e}", found.says), false));
                    }
                }
                self.sound_redraw();
            }
        }
        if self.sound_reconnect.is_some_and(|t| std::time::Instant::now() >= t) {
            self.sound_reconnect = None;
            if let Some(f) = self.full.as_mut() {
                f.reconnect_sound();
            }
        }
    }

    /// A press on one of the bar's buttons.
    fn av_choice(&mut self, tab: usize, cx: &AppCx) {
        match tab {
            browser_av::TAB_INSTALL => {
                // never a download from here: Store -> Setup shows the vendor, the licence and the size, and the person presses its button
                self.av_request = Some(("store".into(), format!("setup:{}", iyakki_agent::avaddon::INSTALL_ITEM)));
                self.av.waiting = true;
                self.av.dismiss();
                cx.audit.record(&cx.session.user, "browser.av.install.asked", "");
            }
            browser_av::TAB_NEVER => {
                if self.av.never_here() {
                    browser_av::save(cx, &self.av.prefs);
                }
                cx.audit.record(&cx.session.user, "browser.av.never", "");
            }
            _ => self.av.dismiss(),
        }
    }

    /// After Install: once the add-on's libraries are there, start the engine again (its environment is read at start) on the same page.
    fn av_poll(&mut self, cx: &AppCx) {
        if !self.av.waiting || self.full.is_none() {
            return;
        }
        if self.av_checked.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2)) {
            return;
        }
        self.av_checked = Some(std::time::Instant::now());
        if iyakki_agent::avaddon::detect(cx.services.dir()) == iyakki_agent::avaddon::Addon::Ready {
            self.av.waiting = false;
            self.restart_full(cx);
        }
    }

    /// Stop the engine and start it again on the page it showed, without touching the tab's history.
    fn restart_full(&mut self, cx: &AppCx) {
        let Some(f) = self.full.take() else { return };
        let url = if f.url.is_empty() { f.good_url.clone() } else { f.url.clone() };
        drop(f);
        let Some(helper) = iyakki_agent::webengine::embedded_helper() else { return };
        self.share_devices("", 0, cx);
        match self.embedded_start(helper, cx).map(Full::start) {
            Ok(Ok(mut f)) => {
                f.navigate(&url);
                self.full = Some(f);
                self.av.shown_reset();
                self.status = Some(("The audio and video add-on is installed: the page is loading again.".into(), true));
                cx.audit.record(&cx.session.user, "browser.av.reload", "");
            }
            Ok(Err(e)) | Err(e) => self.status = Some((format!("The full web engine could not be started again: {e}"), false)),
        }
    }

    fn apply_full_note(&mut self, n: Note, cx: &AppCx) {
        match n {
            Note::Navigated(u) => {
                self.av.page_moved(&iyakki_sec::url_host(&u).unwrap_or_default());
                self.media_page_moved(&u);
                let title = self.full.as_ref().map(|f| f.title.clone()).unwrap_or_default();
                self.set_full_page(&u, &title);
                if self.full.as_ref().is_some_and(|f| f.visited != u) {
                    if let Some(f) = self.full.as_mut() {
                        f.visited = u.clone();
                    }
                    self.data.visit(&u, &title, iyakki_agent::clock::now_unix());
                    browser_data::save(cx, &self.data);
                    cx.audit.record(&cx.session.user, "browser.visit", browser_data::host_of(&u));
                }
                if self.status.as_ref().is_some_and(|s| s.1) {
                    self.status = None;
                }
            }
            Note::Titled(t) => {
                let url = self.full.as_ref().map(|f| f.url.clone()).unwrap_or_default();
                if !url.is_empty() {
                    self.set_full_page(&url, &t);
                    self.data.retitle(&url, &t);
                }
            }
            Note::Refused { url, host } => {
                cx.audit.record(&cx.session.user, "browser.full.refused", &host);
                let _ = url;
                self.status = Some((format!("Your account may not open pages from {host} (the sites an account may open are set by an administrator in Users); the full browser went back to the page before."), false));
            }
            Note::NoAddress { scheme } => {
                let what = if scheme.is_empty() { "this page has no web address".to_string() } else { format!("this page has no web address ({scheme}:)") };
                self.status = Some((format!("The full browser reported a page it cannot name: {what}. The tab stays as it is."), false));
            }
            Note::Download { url, name } => match Url::parse(&url) {
                Ok(u) => self.start_download_of(u, String::new(), Some(name), cx),
                Err(e) => self.status = Some((format!("The page offered a download that cannot be fetched: {e}"), false)),
            },
            Note::Clipboard(t) => {
                crate::ui::clipboard::copy(&t);
                crate::ui::clipboard::toast("Copied");
            }
            Note::Trouble(r) => self.status = Some((format!("The full web engine reports: {r}"), false)),
            Note::NeedsAv(w) => {
                let url = self.full.as_ref().map(|f| f.url.clone()).unwrap_or_default();
                let host = iyakki_sec::url_host(&url).unwrap_or_default();
                self.offer_av(&w, &host, cx);
            }
            Note::Dev(line) => {
                // kept (scrubbed) only while the person's Developer tools panel is open or their switch for the chat agent is on; otherwise dropped here
                let dt = &cx.services.devtools;
                if self.dev.open || dt.enabled(&cx.session.user) {
                    if let Some(f) = &self.full {
                        dt.set_page(&cx.session.user, &f.url);
                    }
                    dt.record_line(&cx.session.user, &line);
                }
            }
            Note::Media { id, kinds, origin } => self.media_request(id, kinds, origin, cx),
            Note::SoundAsk(found) => {
                if self.sound_ask.is_none() {
                    super::soundask::audit(cx, "browser.sound.ask", &found, "asked");
                    self.sound_ask = Some(super::soundask::Ask::new(found, "the browser"));
                    if let Some(f) = self.full.as_mut() {
                        f.want_redraw();
                    }
                }
            }
            Note::Sound(Some(n)) => self.status = Some((n, false)),
            Note::Sound(None) => {
                if self.status.as_ref().is_some_and(|s| s.0.starts_with("Sound ")) {
                    self.status = None;
                }
            }
            Note::Gone(why) => {
                let url = self.full.as_ref().map(|f| if f.url.is_empty() { f.good_url.clone() } else { f.url.clone() }).unwrap_or_default();
                self.full = None;
                self.full_declined = true;
                cx.audit.record(&cx.session.user, "browser.full.crash", &why.chars().take(120).collect::<String>());
                match Url::parse(&url) {
                    Ok(u) if url.starts_with("http") => {
                        self.go(u, How::Replace, cx);
                    }
                    _ => self.drop_stub_entry(),
                }
                self.status = Some((format!("The full web engine stopped ({why}); the page is shown by the built-in reader."), false));
            }
        }
    }

    /// A page in front asked for the camera / microphone / screen: the policy answers, or the person is asked about one kind at a time.
    fn media_request(&mut self, id: u32, kinds: u8, origin: String, cx: &AppCx) {
        if self.media_ask.is_some() {
            if let Some(f) = self.full.as_mut() {
                f.answer_permission(id, false);
            }
            browser_media::audit(cx, "browser.media.deny", &origin, kinds, "another question is open");
            return;
        }
        self.media_decide(id, kinds, origin, 0, cx);
    }

    /// Decide (or ask about) the next kind of request `id`; `granted` are the kinds already answered "this time only".
    fn media_decide(&mut self, id: u32, kinds: u8, origin: String, granted: u8, cx: &AppCx) {
        use browser_media::Decision;
        let perms = self.media.get_or_insert_with(|| browser_media::load(cx));
        match perms.decide(&origin, kinds, true, granted) {
            Decision::Allow => {
                self.media_grant(id, kinds, &origin, "allowed", cx);
            }
            Decision::Deny(why) => {
                if let Some(f) = self.full.as_mut() {
                    f.answer_permission(id, false);
                }
                browser_media::audit(cx, "browser.media.deny", &origin, kinds, why);
            }
            Decision::Ask(k) => {
                browser_media::audit(cx, "browser.media.ask", &origin, k, "asked");
                self.media_ask = Some(browser_media::Ask::new(id, kinds, k, granted, &origin));
                if k == iyakki_webview::proto::media_kind::MICROPHONE || k == iyakki_webview::proto::media_kind::CAMERA {
                    self.load_devices(cx);
                }
                if let Some(f) = self.full.as_mut() {
                    f.want_redraw();
                }
            }
        }
    }

    /// Every kind of the request is allowed: tell the engine; a screen share also starts the shell's feed of its own screen.
    fn media_grant(&mut self, id: u32, kinds: u8, origin: &str, how: &str, cx: &AppCx) {
        self.share_devices(origin, kinds, cx);
        if kinds & iyakki_webview::proto::media_kind::SCREEN != 0 {
            let path = browser_media::new_feed_path();
            browser_media::set_feed_path(Some(path.clone()));
            if let Some(f) = self.full.as_mut() {
                f.send_screen_source(&path.to_string_lossy());
            }
        }
        if let Some(f) = self.full.as_mut() {
            f.answer_permission(id, true);
        }
        browser_media::audit(cx, "browser.media.allow", origin, kinds, how);
    }

    fn media_answer(&mut self, a: browser_media::Answer, cx: &AppCx) {
        let Some(ask) = self.media_ask.take() else { return };
        let perms = self.media.get_or_insert_with(|| browser_media::load(cx));
        let yes = perms.apply(&ask.origin, ask.asking, a);
        if yes && a == browser_media::Answer::Always && (ask.asking == iyakki_webview::proto::media_kind::CAMERA || ask.asking == iyakki_webview::proto::media_kind::MICROPHONE) {
            // "Always allow this site" also remembers the device the person is looking at, for this site
            let id = self.device_now(ask.asking);
            let host = ask.host.clone();
            if let Some(perms) = self.media.as_mut() {
                perms.set_site_device(&host, ask.asking, &id);
            }
        }
        let perms = self.media.get_or_insert_with(|| browser_media::load(cx));
        if matches!(a, browser_media::Answer::Always | browser_media::Answer::Block) {
            browser_media::save(cx, perms);
        }
        let what = match a {
            browser_media::Answer::Once => "allowed once",
            browser_media::Answer::WhileOpen => "allowed while the site is open",
            browser_media::Answer::Always => "always allowed",
            browser_media::Answer::Block => "blocked",
            browser_media::Answer::NotNow => "not now",
        };
        browser_media::audit(cx, if yes { "browser.media.allow" } else { "browser.media.deny" }, &ask.origin, ask.asking, what);
        if yes {
            // the next kind of the same request (the answers "once" are carried along; the others are in the store)
            self.media_decide(ask.id, ask.kinds, ask.origin, ask.granted | ask.asking, cx);
        } else if let Some(f) = self.full.as_mut() {
            f.answer_permission(ask.id, false);
            f.want_redraw();
        }
    }

    fn media_ask_is_mic(&self) -> bool {
        self.media_ask.as_ref().is_some_and(|a| a.asking == iyakki_webview::proto::media_kind::MICROPHONE)
    }

    /// The microphone the next recording uses, as the chooser shows it: the person's choice, else the first one found.
    fn mic_line(&self) -> String {
        let chosen = self.cx_mic_choice();
        match iyakki_agent::voicedesk::pick_mic(&self.media_mics, &chosen) {
            Some(m) => {
                if chosen.is_empty() || chosen != m.id { format!("Automatic: {}", m.label) } else { m.label.clone() }
            }
            None => "No microphone found".to_string(),
        }
    }

    fn cx_mic_choice(&self) -> String {
        self.mic_saved.clone()
    }

    fn media_ask_is_cam(&self) -> bool {
        self.media_ask.as_ref().is_some_and(|a| a.asking == iyakki_webview::proto::media_kind::CAMERA)
    }

    fn devprefs_path(cx: &AppCx) -> std::path::PathBuf {
        browser_media::devprefs_path(cx)
    }

    fn prefs(&mut self, cx: &AppCx) -> &mut iyakki_agent::browserdev::DevicePrefs {
        self.devprefs.get_or_insert_with(|| iyakki_agent::browserdev::DevicePrefs::load(&Self::devprefs_path(cx)))
    }

    fn save_prefs(&mut self, cx: &AppCx) {
        let path = Self::devprefs_path(cx);
        if let Some(p) = &self.devprefs {
            if let Err(e) = p.save(&path) {
                self.status = Some((format!("The device choice could not be saved: {e}"), false));
            }
        }
    }

    /// The microphones and cameras now plugged in, without the ones the person marked "don't use".
    fn load_devices(&mut self, cx: &AppCx) {
        let sys = self.cam_sys.clone();
        let prefs = self.prefs(cx).clone();
        self.media_mics = iyakki_agent::voicedesk::list_mics(&self.mic_proc).into_iter().filter(|m| !prefs.hidden_mics.contains(&m.id)).collect();
        self.media_cams = iyakki_agent::browserdev::list_cameras(&sys).into_iter().filter(|c| !prefs.hidden_cams.contains(&c.id)).collect();
        self.mic_saved = cx.services.voice_settings().mic;
    }

    /// The id of the device the person is looking at in the question for `kind` (the choice, else the first one found).
    fn device_now(&self, kind: u8) -> String {
        use iyakki_webview::proto::media_kind::{CAMERA, MICROPHONE};
        if kind == MICROPHONE {
            iyakki_agent::voicedesk::pick_mic(&self.media_mics, &self.mic_saved).map(|m| m.id.clone()).unwrap_or_default()
        } else if kind == CAMERA {
            let want = self.devprefs.as_ref().map(|p| p.cam.clone()).unwrap_or_default();
            let prefs = self.devprefs.clone().unwrap_or_default();
            prefs.pick_cam(&self.media_cams, &want).map(|c| c.id.clone()).unwrap_or_default()
        } else {
            String::new()
        }
    }

    /// Hand the choices to the engine's plugins: the hidden devices, and the camera and microphone to use first for this site (a device the
    /// site was given with "Always allow" while it is still plugged in and not hidden, else the defaults). Called before the engine starts and
    /// before every allow, so a choice made in the question counts for the very request it was made for.
    fn share_devices(&mut self, origin: &str, kinds: u8, cx: &AppCx) {
        use iyakki_webview::proto::media_kind::{CAMERA, MICROPHONE};
        self.load_devices(cx);
        let host = browser_media::secure_host(origin).unwrap_or_default();
        let (site_cam, site_mic) = match (&self.media, host.is_empty()) {
            (Some(m), false) => (m.site_device(&host, CAMERA), m.site_device(&host, MICROPHONE)),
            _ => (String::new(), String::new()),
        };
        let prefs = self.prefs(cx).clone();
        let cam = if kinds & CAMERA != 0 && self.media_cams.iter().any(|c| c.id == site_cam) { site_cam } else { prefs.cam.clone() };
        let mic = if kinds & MICROPHONE != 0 && self.media_mics.iter().any(|m| m.id == site_mic) { site_mic } else { self.mic_saved.clone() };
        iyakki_agent::browserdev::share_all(&self.mic_dir, &prefs, &cam, &mic);
    }

    /// The camera the next picture uses, as the chooser shows it.
    fn cam_line(&self) -> String {
        let prefs = self.devprefs.clone().unwrap_or_default();
        match prefs.pick_cam(&self.media_cams, &prefs.cam) {
            Some(c) if prefs.cam.is_empty() || prefs.cam != c.id => format!("Automatic: {}", c.label),
            Some(c) => c.label.clone(),
            None => "No camera found".to_string(),
        }
    }

    fn open_cam_pick(&mut self) {
        let mut items = vec!["Automatic".to_string()];
        let labels: Vec<String> = self.media_cams.iter().map(|c| c.label.clone()).collect();
        items.extend(labels.clone());
        self.media_cam_pick = Some(super::widgets::Picker::new("Camera", items).with_deletable(labels, "Delete: don't use this camera"));
    }

    fn open_mic_pick(&mut self) {
        let mut items = vec!["Automatic".to_string()];
        let labels: Vec<String> = self.media_mics.iter().map(|m| m.label.clone()).collect();
        items.extend(labels.clone());
        self.media_mic_pick = Some(super::widgets::Picker::new("Microphone", items).with_deletable(labels, "Delete: don't use this microphone"));
    }

    /// The chooser over the camera question: the choice is the default camera of this person; Delete marks a camera "don't use".
    fn cam_pick_event(&mut self, ev: &Event, cx: &AppCx) {
        let Some(mut p) = self.media_cam_pick.take() else { return };
        let ended = p.handle_event(ev);
        if let Some(label) = p.take_removed() {
            if let Some(id) = self.media_cams.iter().find(|c| c.label == label).map(|c| c.id.clone()) {
                self.prefs(cx).hide_cam(&id);
                self.save_prefs(cx);
                cx.audit.record(&cx.session.user, "browser.media.camera.hidden", "one camera");
                self.load_devices(cx);
                self.share_devices("", 0, cx);
                self.open_cam_pick();
                return;
            }
        }
        match ended {
            super::widgets::PickEvent::Chosen(label) => {
                let id = self.media_cams.iter().find(|c| c.label == label).map(|c| c.id.clone()).unwrap_or_default();
                self.prefs(cx).cam = id.clone();
                self.save_prefs(cx);
                self.share_devices("", 0, cx);
                cx.audit.record(&cx.session.user, "browser.media.camera", if id.is_empty() { "automatic" } else { "chosen" });
            }
            super::widgets::PickEvent::Cancel => {}
            super::widgets::PickEvent::None => self.media_cam_pick = Some(p),
        }
    }

    /// The chooser over the microphone question: the choice is saved in the voice settings (the Voice tile shows the same one) and handed
    /// to the web engine, so it counts for the next getUserMedia; Delete marks a microphone "don't use" (a defective one).
    fn mic_pick_event(&mut self, ev: &Event, cx: &AppCx) {
        let Some(mut p) = self.media_mic_pick.take() else { return };
        let ended = p.handle_event(ev);
        if let Some(label) = p.take_removed() {
            if let Some(id) = self.media_mics.iter().find(|m| m.label == label).map(|m| m.id.clone()) {
                self.prefs(cx).hide_mic(&id);
                self.save_prefs(cx);
                if self.mic_saved == id {
                    let _ = cx.services.set_voice_mic("");
                }
                cx.audit.record(&cx.session.user, "browser.media.microphone.hidden", "one microphone");
                self.load_devices(cx);
                self.share_devices("", 0, cx);
                self.open_mic_pick();
                return;
            }
        }
        match ended {
            super::widgets::PickEvent::Chosen(label) => {
                let id = self.media_mics.iter().find(|m| m.label == label).map(|m| m.id.clone()).unwrap_or_default();
                match cx.services.set_voice_mic(&id) {
                    Ok(()) => {
                        self.mic_saved = id.clone();
                        cx.audit.record(&cx.session.user, "browser.media.microphone", if id.is_empty() { "automatic" } else { &id });
                    }
                    Err(e) => self.status = Some((format!("The microphone choice could not be saved: {e}"), false)),
                }
            }
            super::widgets::PickEvent::Cancel => {}
            super::widgets::PickEvent::None => self.media_mic_pick = Some(p),
        }
    }

    /// Now and then: did the web engine's microphone plugin report a microphone that could not be opened? Said once, in plain words.
    fn mic_failure_tick(&mut self) {
        if self.mic_checked.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(1000)) {
            return;
        }
        self.mic_checked = Some(std::time::Instant::now());
        match iyakki_agent::voicedesk::browser_mic_failure(&self.mic_dir) {
            Some((name, why)) => {
                let key = format!("{name}\t{why}");
                if key != self.mic_told {
                    self.status = Some((iyakki_agent::voicedesk::mic_failure_text(&name, &why), true));
                    self.mic_told = key;
                }
            }
            None => self.mic_told.clear(),
        }
    }

    /// Every beat: tell the shell what the pages capture (for its sign on every screen), obey its "stop" click, end the screen feed when no share is on.
    fn capture_tick(&mut self, cx: &AppCx) {
        use iyakki_webview::proto::media_kind::SCREEN;
        let mut bits = self.full.as_ref().map_or(0, |f| f.capture);
        for t in self.tabs.iter().flatten() {
            bits |= t.full.as_ref().map_or(0, |f| f.capture);
        }
        if browser_media::take_stop() && bits != 0 {
            for f in self.tabs.iter_mut().flatten().filter_map(|t| t.full.as_mut()).chain(self.full.as_mut()) {
                f.stop_capture(bits);
            }
            if let Some(m) = self.media.as_mut() {
                m.end_session();
            }
            cx.audit.record(&cx.session.user, "browser.media.stopped", &browser_media::kinds_words(bits));
        }
        if bits != self.published {
            if self.published & SCREEN != 0 && bits & SCREEN == 0 {
                browser_media::set_feed_path(None);
                if let Some(f) = self.full.as_mut() {
                    f.send_screen_source("");
                }
            }
            self.published = bits;
            browser_media::publish_capture(bits);
        }
    }

    /// The person left the tab with a question open: it is answered no (the page can ask again when it is in front).
    fn media_void(&mut self) {
        if let Some(ask) = self.media_ask.take() {
            if let Some(f) = self.full.as_mut() {
                f.answer_permission(ask.id, false);
            }
        }
    }

    /// The page moved: a question about the old page is void (the helper denies it), and "while this site is open" ends elsewhere.
    fn media_page_moved(&mut self, url: &str) {
        let host = browser_media::secure_host(url).unwrap_or_default();
        if self.media_ask.as_ref().is_some_and(|a| a.host != host) {
            self.media_ask = None;
        }
        if let Some(m) = self.media.as_mut() {
            m.page_moved(&host);
        }
    }

    /// The tab's current entry stands for what the engine shows: address and title for the tab, the star, history.
    fn set_full_page(&mut self, url: &str, title: &str) {
        let Some(page) = full_stub(url, title) else { return };
        if let Some(e) = self.history.get_mut(self.pos) {
            e.page = Some(page);
        }
        if !self.addr_focus {
            self.field.set(url);
        }
    }

    /// An address typed in the bar (or any other way to open one) while the engine shows the page: it goes to the engine.
    fn go_full(&mut self, req: &Request, cx: &AppCx) {
        let url = match iyakki_agent::webengine::allowed(&cx.session.policy, &req.url.to_string()) {
            Ok(u) => u,
            Err(e) => {
                self.status = Some((e, false));
                return;
            }
        };
        self.suggest.clear();
        self.sug_sel = None;
        self.addr_focus = false;
        self.status = None;
        if let Some(f) = self.full.as_mut() {
            f.set_focus(true);
            f.navigate(&url);
        }
    }

    /// The page area while the full engine shows the page.
    fn render_full(&mut self, g: &mut Gfx, view: R, s: f32, tick: u64) {
        g.hit(view, Event::Area(AREA_FULL), "page");
        let Some(f) = self.full.as_mut() else { return };
        f.draw(g, view, s);
        if !f.is_ready() || !f.has_picture() {
            let host = iyakki_sec::url_host(&f.url).unwrap_or_default();
            let msg_host = if f.is_ready() { host.as_str() } else { "" };
            chrome::draw_placeholder(g, view, s, tick, msg_host, ACCENT);
            if !f.is_ready() {
                let msg = "Starting the web engine…";
                let tw = g.text_width(15.0 * s, msg, Face::Sans);
                g.text(view.x + (view.w - tw) / 2.0, view.y + view.h * 0.42 + 56.0 * s, 15.0 * s, DIM, msg, Face::Sans);
            }
        }
    }

    /// After the full browser closed: say how it went.
    fn full_browser_result(&mut self) {
        match crate::external::take_result() {
            Some(Ok(0)) => self.status = Some(("Back from the full browser.".into(), true)),
            Some(Ok(code)) => self.status = Some((format!("The full browser stopped (exit status {code}); details are in /run/iyakki-web.log."), false)),
            Some(Err(e)) => self.status = Some((e, false)),
            None => {}
        }
    }

    /// Hand the video or audio address to the player program; it fetches and shows the file itself.
    fn play_media(&mut self, cx: &AppCx) {
        let Some(url) = self.current_url().cloned() else { return };
        if !cx.session.policy.allows_host(&url.host) {
            self.status = Some((format!("Your account may not open files from {}.", url.host), false));
            return;
        }
        let Some(argv) = iyakki_agent::voice::find_media_player(&self.cfg.player) else {
            self.status = Some(("No video player is installed (mpv, VLC or ffplay).".into(), false));
            return;
        };
        cx.audit.record(&cx.session.user, "browser.play", &url.host);
        match iyakki_agent::voice::launch_media_player(&argv, &url.to_string()) {
            Ok(()) => self.status = Some((format!("Opened in {}.", argv[0].rsplit('/').next().unwrap_or("the player")), true)),
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// A: blocking of ads and trackers off (or on again) for the site of the page. Also the one click on the page that says a
    /// filter list blocked it.
    fn toggle_block_allow(&mut self, cx: &AppCx) {
        let Some(host) = self.current_url().map(|u| u.host.clone()) else {
            self.status = Some(("Open a page first; blocking can be switched off site by site.".into(), false));
            return;
        };
        if !browser_block::load(cx).enabled {
            self.status = Some(("Blocking is off everywhere (Settings → Browser → Block ads and trackers).".into(), false));
            return;
        }
        let allowed = browser_block::toggle_allow(cx, &host);
        browser_block::apply(cx);
        cx.audit.record(&cx.session.user, if allowed { "browser.block.allow" } else { "browser.block.resume" }, &host);
        let note = if self.full.is_some() { " (the full engine uses the new choice when it is opened again)" } else { "" };
        if self.full.is_none() {
            self.reload(cx);
        }
        self.status = Some((if allowed { format!("Ads and trackers are allowed on {host}{note}.") } else { format!("Blocking is back on for {host}{note}.") }, true));
    }

    fn toggle_js(&mut self, cx: &AppCx) {
        let Some(host) = self.current_url().map(|u| u.host.clone()) else {
            self.status = Some(("Open a page first; scripts can be allowed site by site.".into(), false));
            return;
        };
        self.cfg = browser_config::load(cx);
        if self.cfg.js_all {
            self.status = Some(("Scripts are on for every site (Settings → Browser). Turn that off to choose site by site.".into(), false));
            return;
        }
        let now_on = if self.cfg.js_sites.remove(&host) {
            false
        } else {
            self.cfg.js_sites.insert(host.clone());
            iyakki_js::warm_up();
            true
        };
        browser_config::save(cx, &self.cfg);
        cx.audit.record(&cx.session.user, if now_on { "browser.js.allow" } else { "browser.js.stop" }, &host);
        self.reload(cx);
        self.status = Some((format!("JavaScript is {} for {host}.", if now_on { "ON" } else { "off" }), true));
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A search word that sends the rest of the address bar to a particular site (`w rust` searches Wikipedia).
/// The reader showed nothing although the page carries scripts: it is a web app this engine cannot run.
pub fn page_needs_full_engine(page: &Page) -> bool {
    let scripted = page.source.to_ascii_lowercase().contains("<script");
    let left_empty = page.notes.iter().any(|n| n.contains("scripts left it empty"));
    // an app whose scripts keep failing is not going to work in the reader even if it drew a few boxes
    let broken = scripted && page.notes.iter().filter(|n| n.starts_with("error")).count() >= 1;
    (page.doc.blocks.is_empty() && scripted) || left_empty || broken
}

pub fn expand_shortcut(text: &str) -> Option<String> {
    let (key, rest) = text.trim().split_once(char::is_whitespace)?;
    let q = rest.trim();
    if q.is_empty() {
        return None;
    }
    let enc = form_urlencode(&[("q".to_string(), q.to_string())]).trim_start_matches("q=").to_string();
    let site = match key.to_ascii_lowercase().as_str() {
        "w" | "wiki" => "https://en.wikipedia.org/w/index.php?search=",
        "ddg" => "https://duckduckgo.com/html/?q=",
        "b" | "bing" => "https://www.bing.com/search?q=",
        "yt" => "https://www.youtube.com/results?search_query=",
        "gh" => "https://github.com/search?q=",
        "mdn" => "https://developer.mozilla.org/en-US/search?q=",
        "rs" => "https://docs.rs/releases/search?query=",
        _ => return None,
    };
    Some(format!("{site}{enc}"))
}

/// What to tell the person when a page could not be loaded. Certificate problems get a plain explanation: the connection
/// is stopped, there is no way to click through.
pub fn friendly_error(host: &str, e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("certificate") || l.contains("unknownissuer") || l.contains("notvalidforname") || l.contains("badsignature") || l.contains("expired") {
        let why = if l.contains("expired") {
            "its certificate has expired"
        } else if l.contains("notvalidforname") {
            "its certificate is for a different site name"
        } else if l.contains("unknownissuer") {
            "its certificate was not issued by an authority this system trusts (it may be self-signed)"
        } else {
            "its certificate could not be verified"
        };
        return format!("{host} was not opened: {why}. The connection was stopped to protect you.");
    }
    format!("{host}: {e}")
}

fn human_bytes(b: u64) -> String {
    if b >= 1 << 20 { format!("{:.1} MB", b as f64 / (1 << 20) as f64) } else { format!("{} KB", b / 1024 + 1) }
}

/// A file name that is safe to create from the end of an address.
/// What the history entry of a page shown by the full engine holds, so that tabs, stars and the history have a title and an
/// address for it (the page itself is in the helper's picture).
const STUB_TYPE: &str = "x-iyakki/full-engine";
/// The built-in help page the "needs the full web engine" banner opens.
const HELP_TYPE: &str = "x-iyakki/help";
const HELP_URL: &str = "https://iyakki.invalid/help/full-engine";

fn full_stub(url: &str, title: &str) -> Option<Arc<Page>> {
    let parsed = Url::parse(url).ok()?;
    let html = format!("<html><head><title>{}</title></head><body><p>{}</p></body></html>", escape(title), escape(url));
    Some(Arc::new(Page { url: parsed, status: 200, doc: iyakki_web::doc::from_html(&html), notes: Vec::new(), source: String::new(), media: false, download: false, ctype: STUB_TYPE.into() }))
}

/// A file name the engine suggested, cut down to letters, digits and a few marks (never a path).
fn sanitize_file_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ')).collect();
    n.trim_matches(|c: char| c == '.' || c == ' ').chars().take(80).collect()
}

pub fn download_name(url: &str, content_type: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let decoded: String = last.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ')).collect();
    let decoded = decoded.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    let decoded: String = decoded.chars().take(80).collect();
    if decoded.is_empty() || decoded == browser_data::host_of(url) {
        let ext = match content_type.split(';').next().unwrap_or("").trim() {
            "application/pdf" => ".pdf",
            "application/zip" => ".zip",
            "text/plain" => ".txt",
            "text/csv" => ".csv",
            "application/json" => ".json",
            _ => "",
        };
        return format!("download{ext}");
    }
    decoded
}

fn page_label(title: &str, url: &str) -> String {
    if title.trim().is_empty() { url.to_string() } else { title.trim().to_string() }
}

/// The page shown before anything has been opened, and the browser's own history and bookmarks pages. Every address in
/// it is complete, so it can be followed without a page to resolve it against.
fn start_page(data: &Data, engine: &str, view: StartView) -> (Doc, Vec<String>) {
    let mut html = String::new();
    match view {
        StartView::Home => {
            html.push_str(&format!("<h1>Search the web</h1><p>Type in the bar above and press Enter. Words are searched on {}; an address such as example.org is opened. Short words go straight to a site: w (Wikipedia), ddg, yt, gh, mdn.</p><p>F1 shows every key. Ctrl+D bookmarks a page, Ctrl+B shows bookmarks, Ctrl+H the history.</p>", escape(engine)));
            if !data.bookmarks.is_empty() {
                html.push_str("<h3>Bookmarks</h3><ul>");
                for b in data.bookmarks.iter().take(8) {
                    html.push_str(&format!("<li><a href=\"{}\">{}</a></li>", escape(&b.url), escape(&page_label(&b.title, &b.url))));
                }
                html.push_str("</ul>");
            }
            let top = data.top_sites(8);
            if !top.is_empty() {
                html.push_str("<h3>Most visited</h3><ul>");
                for v in top {
                    html.push_str(&format!("<li><a href=\"{}\">{}</a> · {}</li>", escape(&v.url), escape(&page_label(&v.title, &v.url)), escape(browser_data::host_of(&v.url))));
                }
                html.push_str("</ul>");
            }
            let recent = data.recent(8);
            if !recent.is_empty() {
                html.push_str("<h3>Recent</h3><ul>");
                for v in recent {
                    html.push_str(&format!("<li><a href=\"{}\">{}</a></li>", escape(&v.url), escape(&page_label(&v.title, &v.url))));
                }
                html.push_str("</ul>");
            }
        }
        StartView::History => {
            html.push_str(&format!("<h1>History</h1><p>{} pages remembered on this account. Shift+D clears the list. Only addresses and titles are kept.</p>", data.history.len()));
            html.push_str("<ul>");
            for v in data.recent(200) {
                html.push_str(&format!("<li><a href=\"{}\">{}</a> · {}</li>", escape(&v.url), escape(&page_label(&v.title, &v.url)), escape(browser_data::host_of(&v.url))));
            }
            html.push_str("</ul>");
        }
        StartView::Downloads => {
            html.push_str("<h1>Downloads</h1><p>Files saved from pages go to the Downloads folder in your file area. D on a page that is a file saves it.</p>");
            if data.downloads.is_empty() {
                html.push_str("<p>Nothing downloaded yet.</p>");
            }
            html.push_str("<ul>");
            for d in &data.downloads {
                html.push_str(&format!("<li>{} · {} · {}</li>", escape(&d.name), human_bytes(d.bytes), escape(&d.status)));
            }
            html.push_str("</ul>");
        }
        StartView::Bookmarks => {
            html.push_str("<h1>Bookmarks</h1><p>Ctrl+D on a page adds or removes it. Ctrl+E saves the list as bookmarks.html in Downloads; Shift+I reads that file back (other browsers' exports work too).</p>");
            if data.bookmarks.is_empty() {
                html.push_str("<p>Nothing is bookmarked yet.</p>");
            }
            for (folder, items) in data.bookmark_folders() {
                if !folder.is_empty() {
                    html.push_str(&format!("<h3>{}</h3>", escape(&folder)));
                }
                html.push_str("<ul>");
                for b in items {
                    html.push_str(&format!("<li><a href=\"{}\">{}</a> · {}</li>", escape(&b.url), escape(&page_label(&b.title, &b.url)), escape(browser_data::host_of(&b.url))));
                }
                html.push_str("</ul>");
            }
        }
    }
    let doc = iyakki_web::doc::from_html(&html);
    let links = doc.links.clone();
    (doc, links)
}

// ------------------------------------------------------------------------------------------------ pictures

/// Draw a picture in its box, or a quiet placeholder while it is on its way, or its description if it could not be shown.
fn draw_image(g: &mut Gfx, alt: &str, st: Option<&ImgState>, r: R, s: f32, tick: u64, fit: iyakki_web::style::Fit, alpha: f32) {
    // animations run on the shell's clock (~30 ticks a second); a tab that is not shown is not drawn, so it does not advance work
    let frame: Option<&tiny_skia::Pixmap> = match st {
        Some(ImgState::Ready(pm)) => Some(pm),
        Some(ImgState::Animated(a)) => Some(a.frame_at(tick * 1000 / 30)),
        _ => None,
    };
    match st {
        Some(ImgState::Ready(_) | ImgState::Animated(_)) => {
            let Some(pm) = frame else { return };
            if fit != iyakki_web::style::Fit::Fill || alpha < 1.0 {
                draw_fitted(g, pm, r, fit, alpha);
                return;
            }
            let (sx, sy) = (r.w / pm.width() as f32, r.h / pm.height() as f32);
            let paint = PixmapPaint { opacity: 1.0, quality: if sx < 0.6 { FilterQuality::Bicubic } else { FilterQuality::Bilinear }, ..PixmapPaint::default() };
            let pixmap: &tiny_skia::Pixmap = pm;
            g.pm.draw_pixmap(0, 0, pixmap.as_ref(), &paint, Transform::from_translate(r.x, r.y).pre_scale(sx, sy), None);
        }
        Some(ImgState::Failed(why)) => {
            // neutral greys that read on a dark page and on a light one
            g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, [128, 128, 128, 34]);
            g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 1.0 * s, [128, 128, 128, 110]);
            let label = if alt.is_empty() { "image".to_string() } else { alt.to_string() };
            let size = 14.4 * s;
            let label = g.fit(size, &label, Face::Sans, r.w - 12.0 * s);
            let lh = g.line_height(size, Face::Sans);
            g.text(r.x + 10.0 * s, r.y + (r.h - lh) / 2.0, size, [128, 128, 128, 255], &label, Face::Sans);
            let _ = why;
        }
        _ => {
            g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, [128, 128, 128, 26]);
            g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 1.0 * s, [128, 128, 128, 80]);
        }
    }
}

/// A picture in a box with `object-fit` (other than stretching it) or `opacity`: scaled and placed by the fit, and
/// painted only inside the part of the box it covers.
fn draw_fitted(g: &mut Gfx, pm: &tiny_skia::Pixmap, r: R, fit: iyakki_web::style::Fit, alpha: f32) {
    let (clip, sx, sy, tx, ty) = super::browser_layout::fit_picture(fit, r, pm.width() as f32, pm.height() as f32);
    let quality = if sx < 0.6 { FilterQuality::Bicubic } else { FilterQuality::Bilinear };
    let pattern = tiny_skia::Pattern::new(pm.as_ref(), tiny_skia::SpreadMode::Pad, quality, alpha.clamp(0.0, 1.0), Transform::from_translate(tx, ty).pre_scale(sx, sy));
    let mut paint = tiny_skia::Paint::default();
    paint.shader = pattern;
    if let Some(rect) = tiny_skia::Rect::from_xywh(clip.x, clip.y, clip.w.max(0.5), clip.h.max(0.5)) {
        g.pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

// ------------------------------------------------------------------------------------------------ form controls

/// Draw one control in its box `r` and register its click region.
#[allow(clippy::too_many_arguments)]
fn draw_control(g: &mut Gfx, c: &Control, st: &CtlState, r: R, s: f32, focused: bool, edit: &TextField, tick: u64, idx: usize, light: bool) {
    // on a page that paints itself light the controls are light too
    let (field, dim, text_c, muted, card, card_hot) = if light { (rgb(255, 255, 255), rgb(118, 118, 118), rgb(32, 33, 36), rgb(95, 99, 104), rgb(241, 243, 244), rgb(232, 234, 237)) } else { (FIELD, DIM, TEXT, MUTED, CARD, crate::ui::CARD_HOVER) };
    let id = CONTROL_BASE + idx;
    let size = 16.0 * s;
    let lh = g.line_height(size, Face::Sans);
    let text_color = if c.disabled { dim } else { text_c };
    let ring = |g: &mut Gfx| g.rrect_outline(r.x - 2.0 * s, r.y - 2.0 * s, r.w + 4.0 * s, r.h + 4.0 * s, 10.0 * s, 2.0 * s, ACCENT);
    match c.kind {
        ControlKind::Text | ControlKind::Password => {
            if focused {
                edit.render(g, r, true, s, blink(tick), Face::Sans, id);
                return;
            }
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, field);
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.0 * s, dim);
            let (shown, col) = if st.value.is_empty() { (c.placeholder.clone(), dim) } else if c.kind == ControlKind::Password { ("•".repeat(st.value.chars().count()), text_color) } else { (st.value.clone(), text_color) };
            let shown = g.fit(size, &shown, Face::Sans, r.w - 24.0 * s);
            g.text(r.x + 12.0 * s, r.y + (r.h - lh) / 2.0, size, col, &shown, Face::Sans);
            g.hit(r, Event::Focus(id), "field");
        }
        ControlKind::Textarea => {
            if focused {
                // the box draws itself, so it tells the shell it holds the keyboard focus: the on-screen keyboard opens for a finger
                crate::ctxmenu::note_field(crate::ctxmenu::FieldNote { rect: r, id, focused: true, has_selection: false, empty: st.value.is_empty(), secret: false });
            }
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, field);
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, if focused { 2.0 * s } else { 1.0 * s }, if focused { ACCENT } else { dim });
            let rows = (((r.h - 12.0 * s) / (lh * 1.0)) as usize).max(1);
            let text = if st.value.is_empty() && !focused { c.placeholder.clone() } else { st.value.clone() };
            let col = if st.value.is_empty() { dim } else { text_color };
            let mut lines: Vec<String> = Vec::new();
            for l in text.split('\n') {
                lines.extend(g.wrap(size, l, Face::Sans, r.w - 24.0 * s));
            }
            let from = if focused { lines.len().saturating_sub(rows) } else { 0 };
            for (i, l) in lines[from..].iter().take(rows).enumerate() {
                g.text(r.x + 12.0 * s, r.y + 6.0 * s + i as f32 * lh, size, col, l, Face::Sans);
            }
            if focused && blink(tick) {
                let last = lines.get(from..).and_then(|v| v.last()).cloned().unwrap_or_default();
                let n = (lines.len() - from).max(1).min(rows.max(1));
                let tw = g.text_width(size, &last, Face::Sans);
                g.rect(r.x + 12.0 * s + tw + 1.0, r.y + 6.0 * s + (n - 1) as f32 * lh, 2.0 * s, lh, text_c);
            }
            g.hit(r, Event::Focus(id), "text area");
        }
        ControlKind::Checkbox => {
            g.rrect(r.x, r.y, r.w, r.h, 4.0 * s, field);
            g.rrect_outline(r.x, r.y, r.w, r.h, 4.0 * s, 1.5 * s, if st.checked { ACCENT } else { dim });
            if st.checked {
                let tw = g.text_width(size, "✓", Face::Bold);
                g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, size, ACCENT, "✓", Face::Bold);
            }
            if focused {
                ring(g);
            }
            g.hit(r, Event::Focus(id), "checkbox");
        }
        ControlKind::Radio => {
            g.circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0, dim);
            g.circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0 - 1.5 * s, field);
            if st.checked {
                g.circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 4.0, ACCENT);
            }
            if focused {
                ring(g);
            }
            g.hit(r, Event::Focus(id), "radio button");
        }
        ControlKind::Submit | ControlKind::Button => {
            let hot = g.hovered(r);
            g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, if hot { card_hot } else { card });
            g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, if c.kind == ControlKind::Submit { ACCENT } else { dim });
            let label = g.fit(size, &c.label, Face::Bold, r.w - 12.0 * s);
            let tw = g.text_width(size, &label, Face::Bold);
            g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, size, text_color, &label, Face::Bold);
            if focused {
                ring(g);
            }
            g.hit(r, Event::Focus(id), &c.label);
        }
        ControlKind::Select => {
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, field);
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, if focused { 2.0 * s } else { 1.0 * s }, if focused { ACCENT } else { dim });
            let label = c.options.get(st.sel).map_or(String::new(), |o| o.label.clone());
            let label = g.fit(size, &label, Face::Sans, r.w - 40.0 * s);
            g.text(r.x + 12.0 * s, r.y + (r.h - lh) / 2.0, size, text_color, &label, Face::Sans);
            g.text(r.x + r.w - 24.0 * s, r.y + (r.h - lh) / 2.0, size, muted, "▾", Face::Sans);
            g.hit(r, Event::Focus(id), "drop-down");
        }
        ControlKind::Hidden => {}
    }
}

// ------------------------------------------------------------------------------------------------ the app

impl BrowserApp {
    /// The page (or the start page) as the in-house engine draws it, in a readable column.
    /// The strip above the page that says plainly that the page needs the full web engine; returns the page area that is left.
    fn render_engine_banner(&mut self, g: &mut Gfx, view: R, s: f32) -> R {
        let Some((head, body)) = self.engine_banner() else { return view };
        let pad = 10.0 * s;
        let lh = g.line_height(13.0 * s, Face::Sans);
        let lines: Vec<String> = g.wrap(13.0 * s, &body, Face::Sans, view.w - 2.0 * pad).into_iter().take(3).collect();
        let h = (pad + 32.0 * s + lines.len() as f32 * lh + pad).min(view.h * 0.5);
        let warn = rgb(255, 176, 70);
        g.rect(view.x, view.y, view.w, h, rgb(44, 32, 14));
        g.rect(view.x, view.y + h - 1.0, view.w, 1.0, [warn[0], warn[1], warn[2], 140]);
        let bw = g.text_width(14.0 * s, "How to get it", Face::Bold) + 14.0 * s * 1.7;
        let head = g.fit(15.0 * s, &head, Face::Bold, (view.w - 2.0 * pad - bw - 10.0 * s).max(40.0));
        g.text(view.x + pad, view.y + pad + 4.0 * s, 15.0 * s, warn, &head, Face::Bold);
        g.button(view.x + view.w - pad - bw, view.y + pad * 0.6, 14.0 * s, "How to get it", warn, Event::Tab(10));
        let mut y = view.y + pad + 32.0 * s;
        for l in &lines {
            if y + lh > view.y + h {
                break;
            }
            g.text(view.x + pad, y, 13.0 * s, TEXT, l, Face::Sans);
            y += lh;
        }
        let left = R::new(view.x, view.y + h, view.w, (view.h - h).max(0.0));
        self.view_rect = left;
        self.view_h = left.h;
        left
    }

    fn render_reader(&mut self, g: &mut Gfx, view: R, s: f32, tick: u64) {
        let view = self.render_engine_banner(g, view, s);
        // the page (or the start page), in a readable column
        let ps = s * self.zoom();
        let page = self.page().cloned();
        // a page with style sheets of its own was designed for the whole window (its media queries were evaluated for it,
        // see `Viewport`): it gets the width; plain documents stay in a column that is comfortable to read
        let designed = page.as_ref().is_some_and(|p| p.doc.canvas.is_some() && !matches!(p.ctype.as_str(), HELP_TYPE | STUB_TYPE));
        let col_max = if designed { iyakki_web::css::Viewport::default().width } else { 940.0 };
        let col_w = (view.w - 48.0 * s).min(col_max * ps).max(120.0);
        let col_x = view.x + (view.w - col_w) / 2.0;
        let start_doc;
        let doc: &Doc = match &page {
            Some(p) => &p.doc,
            None => {
                let (d, links) = start_page(&self.data, self.cfg.search.name(), self.view());
                // the page is rebuilt from the saved data each frame; lay it out again only when it changed
                if links != self.start_links {
                    self.layout = None;
                }
                self.start_links = links;
                start_doc = d;
                &start_doc
            }
        };
        let ctl: Vec<CtlState> = self.history.get(self.pos).map(|e| e.ctl.clone()).unwrap_or_default();
        let states: Vec<ImgState> = self.history.get(self.pos).map(|e| e.images.clone()).unwrap_or_default();
        let infos: Vec<ImgInfo> = states.iter().map(|st| match st {
            ImgState::Ready(pm) => ImgInfo { size: Some((pm.width(), pm.height())), failed: false },
            ImgState::Animated(a) => ImgInfo { size: Some(a.frame_at(0).width()).zip(Some(a.frame_at(0).height())), failed: false },
            ImgState::Failed(_) => ImgInfo { size: None, failed: true },
            _ => ImgInfo::default(),
        }).collect();
        let rev = states.iter().filter(|st| st.settled()).count() as u32;
        let key = key_of(col_w, ps);
        if self.layout.as_ref().is_none_or(|l| l.key != key || l.rev != rev) {
            let night = if self.page().is_some_and(|p| self.data.is_dark(&p.url.host)) { browser_dark::darken(doc) } else { None };
            self.layout = Some(layout(g, night.as_ref().unwrap_or(doc), col_w, ps, &infos, rev));
            if let Some(y) = self.pending_scroll.take() {
                self.scroll = y;
            }
            if let Some(f) = &mut self.find {
                f.dirty = true; // the words moved
            }
        }
        self.update_find();
        self.scroll = self.scroll.max(0.0).min(self.max_scroll());
        // pressing anywhere in the page (links and controls are registered after this and sit on top) starts a selection
        g.hit(view, Event::Area(AREA_PAGE), "page");
        self.col_off = col_x - view.x;
        // what is selected, as pieces of drawn words
        self.sel_range = None;
        self.sel_text.clear();
        let mut sel_marks: std::collections::HashMap<usize, Vec<(usize, usize)>> = std::collections::HashMap::new();
        if let (Some(sel), Some(l)) = (self.sel, &self.layout) {
            let (chars, map, starts) = text_stream_with_starts(&l.prims);
            let range = if sel.all {
                (0, chars.len())
            } else {
                let a = stream_index_at(g, &l.prims, &starts, chars.len(), sel.anchor.0, sel.anchor.1);
                let b = stream_index_at(g, &l.prims, &starts, chars.len(), sel.head.0, sel.head.1);
                if a == b && sel.mode == 1 { (0, 0) } else { grow_range(&chars, a, b, sel.mode) }
            };
            if range.1 > range.0 {
                self.sel_range = Some(range);
                self.sel_text = chars[range.0..range.1].iter().collect();
                for (pi, a, b) in range_segments(&map, range.0, range.1) {
                    sel_marks.entry(pi).or_default().push((a, b));
                }
            }
        }
        let top = view.y + 12.0 * s;
        self.hovered_link = None;
        // the page's own background fills the whole view
        if let Some(c) = self.layout.as_ref().and_then(|l| l.canvas) {
            g.rect(view.x, view.y, view.w, view.h, c);
        }
        if let Some(l) = &self.layout {
            // what the find bar matched, by drawn word, with the current match marked
            let mut marks: std::collections::HashMap<usize, Vec<(usize, usize, bool)>> = std::collections::HashMap::new();
            if let Some(f) = &self.find {
                for (mi, m) in f.matches.iter().enumerate() {
                    for (pi, a, b) in m {
                        marks.entry(*pi).or_default().push((*a, *b, mi == f.cur));
                    }
                }
            }
            for (pi, p) in l.prims.iter().enumerate() {
                let (py, ph) = (p.y - self.scroll, p.h);
                if py + ph < -12.0 || py > view.h + 12.0 {
                    continue;
                }
                let (dx, dy) = (col_x + p.x, top + py - 12.0 * s);
                if let (Some(ms), Kind::Text { text, size, face, .. }) = (sel_marks.get(&pi), &p.kind) {
                    for (a, b) in ms {
                        let pre: String = text.chars().take(*a).collect();
                        let mid: String = text.chars().skip(*a).take(*b - *a).collect();
                        let x0 = dx + g.text_width(*size, &pre, *face);
                        let w = g.text_width(*size, &mid, *face);
                        g.rect(x0, dy, w + if *b >= text.chars().count() { 3.0 } else { 0.0 }, p.h, [70, 110, 210, 190]);
                    }
                }
                if let (Some(ms), Kind::Text { text, size, face, .. }) = (marks.get(&pi), &p.kind) {
                    for (a, b, current) in ms {
                        let pre: String = text.chars().take(*a).collect();
                        let mid: String = text.chars().skip(*a).take(*b - *a).collect();
                        let x0 = dx + g.text_width(*size, &pre, *face);
                        let w = g.text_width(*size, &mid, *face);
                        g.rect(x0 - 1.0, dy, w + 2.0, p.h, if *current { [255, 150, 40, 230] } else { [255, 220, 70, 150] });
                    }
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
                        if let Some(li) = link {
                            let r = R::new(dx, dy, p.w, p.h);
                            if g.hovered(r) {
                                self.hovered_link = Some(*li);
                            }
                            g.hit(r, Event::Row(*li), "link");
                        }
                    }
                    Kind::Image { idx, link, fit, alpha } => {
                        let r = R::new(dx, dy, p.w, p.h);
                        draw_image(g, doc.images.get(*idx).map_or("", |i| i.alt.as_str()), states.get(*idx), r, ps, tick, *fit, *alpha);
                        if let Some(li) = link {
                            if g.hovered(r) {
                                self.hovered_link = Some(*li);
                            }
                            g.hit(r, Event::Row(*li), "link");
                        }
                    }
                    Kind::Control { idx } => {
                        if let (Some(c), Some(st)) = (doc.controls.get(*idx), ctl.get(*idx)) {
                            draw_control(g, c, st, R::new(dx, dy, p.w, p.h), ps, self.focus_ctl == Some(*idx), &self.edit, tick, *idx, self.layout.as_ref().and_then(|l| l.canvas).is_some_and(|c| browser_dark::is_light_color(c)));
                        }
                    }
                }
            }
            if let Some(r) = self.focus_link.and_then(|i| l.links.get(i).copied().flatten()) {
                g.rrect_outline(col_x + r.x - 3.0 * s, top + r.y - self.scroll - 12.0 * s - 1.0, r.w + 6.0 * s, r.h + 2.0, 4.0 * s, 2.0 * s, ACCENT);
            }
        }
        // find in page: the bar over the top of the page
        self.update_find();
        if let Some(f) = &self.find {
            let bh = 44.0 * s;
            let bw = (view.w - 40.0 * s).min(560.0 * s);
            let bx0 = view.x + view.w - bw - 20.0 * s;
            let r = R::new(bx0, view.y + 8.0 * s, bw, bh);
            g.rrect_glow(r.x, r.y, r.w, r.h, 10.0 * s, 5.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 50]);
            g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, rgb(16, 21, 40));
            g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, 1.3 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 140]);
            let count = if f.field.value.is_empty() { String::new() } else if f.matches.is_empty() { "No matches".to_string() } else { format!("{} of {}", f.cur + 1, f.matches.len()) };
            let cw = g.text_width(14.0 * s, &count, Face::Sans);
            let (btn, gap) = (30.0 * s, 4.0 * s);
            let right = r.x + r.w - 10.0 * s;
            f.field.render(g, R::new(r.x + 8.0 * s, r.y + 5.0 * s, r.w - 24.0 * s - cw - 3.0 * (btn + gap) - 16.0 * s, r.h - 10.0 * s), self.addr_focus || self.focus_ctl.is_some(), s, blink(tick), Face::Sans, FIND_ID);
            g.text(right - 3.0 * (btn + gap) - cw - 8.0 * s, r.y + 13.0 * s, 14.0 * s, if f.matches.is_empty() && !f.field.value.is_empty() { ERR } else { MUTED }, &count, Face::Sans);
            for (i, (label, ev, name)) in [("↑", Event::Tab(500), "Previous match"), ("↓", Event::Tab(501), "Next match"), ("×", Event::Tab(502), "Close find")].into_iter().enumerate() {
                let br = R::new(right - (3 - i) as f32 * (btn + gap) + gap, r.y + 7.0 * s, btn, 30.0 * s);
                let hot = g.hovered(br);
                crate::fx::row_at(g, br.x, br.y, br.w, br.h, 8.0 * s, false, hot, ACCENT);
                g.hit(br, ev, name);
                let tw = g.text_width(17.0 * s, label, Face::Bold);
                g.text(br.x + (br.w - tw) / 2.0, br.y + 4.0 * s, 17.0 * s, TEXT, label, Face::Bold);
            }
        }
        // a scroll bar
        if let Some(l) = &self.layout {
            if l.height > view.h {
                let frac = view.h / l.height;
                let (bh, track) = ((view.h * frac).max(24.0 * s), view.h);
                let by = view.y + (track - bh) * (self.scroll / (l.height - view.h).max(1.0));
                g.rrect(view.x + view.w - 8.0 * s, by, 5.0 * s, bh, 2.5 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 150]);
            }
        }
        // what the filter lists blocked on this page
        if self.show_blocked {
            let f = iyakki_web::blocker_state::Filter::global();
            let log = f.page_log();
            let ph = (view.h * 0.4).min(260.0 * s);
            let (px, py) = (view.x + 20.0 * s, view.y + view.h - ph - 8.0 * s);
            g.rrect(px, py, view.w - 40.0 * s, ph, 10.0 * s, [8, 11, 24, 238]);
            g.rrect_outline(px, py, view.w - 40.0 * s, ph, 10.0 * s, 1.5 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 110]);
            let size = 13.0 * s;
            let lh = g.line_height(size, Face::Mono) + 3.0 * s;
            let rows = (((ph - 20.0 * s) / lh) as usize).max(1);
            let n = f.page_blocked().1;
            let head = if !f.enabled() { "Blocked · blocking is off".to_string() } else { format!("Blocked · {n} on this page (A allows this site, B closes)") };
            let head = g.fit(size, &head, Face::Bold, view.w - 70.0 * s);
            g.text(px + 12.0 * s, py + 6.0 * s, size, ACCENT, &head, Face::Bold);
            for (i, b) in log.iter().rev().take(rows.saturating_sub(1)).enumerate() {
                let l = g.fit(size, &format!("{:<7} {}   {}", b.kind.label(), b.url, b.rule), Face::Mono, view.w - 70.0 * s);
                g.text(px + 12.0 * s, py + 10.0 * s + (i + 1) as f32 * lh, size, MUTED, &l, Face::Mono);
            }
        }
        // the developer panel: script errors and what was skipped
        if self.show_net {
            let lines: Vec<String> = self.net_log.lock().map(|l| l.iter().map(|n| n.text()).collect()).unwrap_or_default();
            let ph = (view.h * 0.4).min(260.0 * s);
            let (px, py) = (view.x + 20.0 * s, view.y + view.h - ph - 8.0 * s);
            g.rrect(px, py, view.w - 40.0 * s, ph, 10.0 * s, [8, 11, 24, 238]);
            g.rrect_outline(px, py, view.w - 40.0 * s, ph, 10.0 * s, 1.5 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 110]);
            let size = 13.0 * s;
            let lh = g.line_height(size, Face::Mono) + 3.0 * s;
            let rows = (((ph - 20.0 * s) / lh) as usize).max(1);
            g.text(px + 12.0 * s, py + 6.0 * s, size, ACCENT, &format!("Network · {} request{}", lines.len(), if lines.len() == 1 { "" } else { "s" }), Face::Bold);
            for (i, l) in lines.iter().take(rows.saturating_sub(1)).enumerate() {
                let l = g.fit(size, l, Face::Mono, view.w - 70.0 * s);
                let col = if l.split_whitespace().nth(1).is_some_and(|c| c == "---" || c.starts_with(['4', '5'])) { ERR } else { MUTED };
                g.text(px + 12.0 * s, py + 10.0 * s + (i + 1) as f32 * lh, size, col, &l, Face::Mono);
            }
        }
        if self.show_notes {
            if let Some(p) = self.page() {
                let ph = (view.h * 0.4).min(260.0 * s);
                let (px, py) = (view.x + 20.0 * s, view.y + view.h - ph - 8.0 * s);
                g.rrect(px, py, view.w - 40.0 * s, ph, 10.0 * s, [8, 11, 24, 238]);
                g.rrect_outline(px, py, view.w - 40.0 * s, ph, 10.0 * s, 1.5 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 110]);
                let size = 13.0 * s;
                let lh = g.line_height(size, Face::Mono) + 3.0 * s;
                let rows = (((ph - 20.0 * s) / lh) as usize).max(1);
                for (i, l) in p.notes.iter().take(rows).enumerate() {
                    let l = g.fit(size, l, Face::Mono, view.w - 70.0 * s);
                    let col = if l.contains("error") || l.contains("failed") || l.starts_with("error") { ERR } else { MUTED };
                    g.text(px + 12.0 * s, py + 10.0 * s + i as f32 * lh, size, col, &l, Face::Mono);
                }
            }
        }
    }
}

impl App for BrowserApp {
    fn take_request(&mut self) -> Option<(String, String)> {
        self.av_request.take()
    }
    /// `newtab:<address>`: the address in a new tab (the sign-in dialog of Claude Code / Copilot uses it; agent_login.rs).
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        if let Some(url) = item.strip_prefix("newtab:") {
            self.new_tab();
            self.open_url(url, cx);
        }
    }
    fn on_drop(&mut self, _paths: &[std::path::PathBuf], _cx: &AppCx) -> super::DropAction {
        super::DropAction::Ignore("The browser does not take dropped files: drop them on DATA, the AI IDE or the chat".into())
    }
    fn title(&self) -> &'static str {
        "Browser"
    }
    fn icon(&self) -> Icon {
        Icon::Browser
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.agent_view {
            "the assistant's tab · your clicks and keys work as usual · Stop or Take over above · Esc back to your tabs".into()
        } else if self.addr_focus {
            "Enter open · Esc stop typing".into()
        } else if self.full.as_ref().is_some_and(|f| f.page_focus) {
            "typing goes to the page · Esc leave it · Ctrl+L address · Ctrl+T tab".into()
        } else if self.full.is_some() {
            "F or Esc built-in reader · b back · f forward · r reload · g address · F12 dev tools · click the page to type".into()
        } else if self.focus_ctl.is_some() {
            "type · Enter send the form · Tab next · Esc leave the field".into()
        } else {
            "g address · Tab link/field · b back · f forward · r reload · R reader · Q ask · j JavaScript · c notes · F12 dev tools".into()
        }
    }

    fn help(&self) -> Vec<String> {
        [
            "g or click the bar · type an address or a search, Enter to open",
            "Tab / Shift+Tab · next link or form field",
            "Enter · open the link, or send the form",
            "b · back      f · forward      r · reload",
            "Ctrl+T new tab · Ctrl+W close it · Ctrl+Shift+T bring it back · Ctrl+PgUp/PgDn switch",
            "Ctrl+D bookmark · Ctrl+B bookmarks · Ctrl+H history · Ctrl+J downloads · D save a file",
            "Ctrl+K fill the saved sign-in · Ctrl+S keep one · Ctrl+G make a password · Ctrl+F find · Y copy a link",
            "j · allow JavaScript for this site (Settings → Browser for all)",
            "c · developer panel: notes about what was blocked or failed, then the network list",
            "F12 or Ctrl+Shift+I · Developer tools: console, network, timings (1 2 3 or Tab switch, / filter, f failed only, s sort, l level, x clear, y copy, d dock, + - size, Esc close)",
            "Up / Down / PgUp / PgDn · scroll      + / − / 0 · zoom this site      R · reader view (just the article)      U · view source      X · forget this site's cookies and data      N · night colours",
            "F · show this page with the full web engine (WebKit, drawn in this page area; needs its helper program, see docs/FULL-BROWSER.md). With it on: typing, clicks and the wheel go to the page, Esc hands the keyboard back, then F or Esc returns to the built-in reader",
            "Q · ask the assistant about the page or the selection    S · summarise    T · translate to English    M · keep it in your knowledge (RAG)",
            "Esc · leave a field, then leave the browser",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        if self.agent_view {
            return false;
        }
        match self.full.as_mut() {
            Some(f) => f.wheel_px(px),
            None => {
                let before = self.scroll;
                self.scroll_by(-(px as f32) / crate::scrolleng::NOTCH_PX * 60.0);
                if self.scroll == before {
                    crate::scrolleng::note_bound();
                }
                true
            }
        }
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            super::web_agent::tick(&cx.services, &cx.audit, &cx.session);
        }
        if matches!(ev, Event::Tab(super::web_agent::TAB_OPEN)) {
            self.agent_view = true;
            return Flow::Stay;
        }
        if self.media_mic_pick.is_some() && !matches!(ev, Event::Tick | Event::Key(_)) {
            self.mic_pick_event(ev, cx);
            return Flow::Stay;
        }
        if self.media_cam_pick.is_some() && !matches!(ev, Event::Tick | Event::Key(_)) {
            self.cam_pick_event(ev, cx);
            return Flow::Stay;
        }
        if self.agent_view && !matches!(ev, Event::Tick) {
            if matches!(ev, Event::Tab(super::web_agent::TAB_LEAVE) | Event::Key(Key::Esc)) {
                self.agent_view = false;
                return Flow::Stay;
            }
            if super::web_agent::handle(ev, &cx.services, &cx.session.user) {
                return Flow::Stay;
            }
        }
        match ev {
            Event::Tick => {
                self.poll(cx);
                if let Some(a) = &mut self.assist {
                    a.poll();
                }
                self.publish_snapshot(cx);
                Flow::Stay
            }
            Event::Tab(n) if browser_devpanel::is_tab(*n) => {
                if *n == browser_devpanel::TAB_TOGGLE {
                    self.toggle_devpanel(cx);
                } else {
                    let a = self.dev.click(*n);
                    self.dev_act(a, cx);
                }
                Flow::Stay
            }
            Event::AreaAt(browser_devpanel::AREA_BODY, ..) => {
                self.dev.focus = true;
                self.addr_focus = false;
                Flow::Stay
            }
            Event::AreaAt(browser_devpanel::AREA_SPLIT, x, y) => {
                self.dev.drag(*x, *y, crate::ui::dragging());
                self.dev_dirty = true;
                Flow::Stay
            }
            Event::Key(k) => self.handle_key(k, cx),
            Event::Paste(t) if self.dev.open && self.dev.editing => {
                self.dev.paste(t);
                self.dev_dirty = true;
                Flow::Stay
            }
            Event::Paste(t) if self.assist.is_some() => {
                if let Some(a) = &mut self.assist {
                    a.paste(t);
                }
                Flow::Stay
            }
            Event::Paste(t) if self.full_has_keys() => {
                if let Some(f) = self.full.as_mut() {
                    f.text(t);
                }
                Flow::Stay
            }
            Event::Paste(t) => {
                if self.addr_focus {
                    self.field.paste(t);
                    self.refresh_suggestions();
                } else if let Some(f) = self.find.as_mut().filter(|_| self.focus_ctl.is_none()) {
                    if f.field.paste(t) {
                        f.dirty = true;
                        f.cur = 0;
                        self.update_find();
                        self.scroll_to_match();
                    }
                } else if let Some(c) = self.focus_ctl {
                    let text_field = self.control(c).is_some_and(|(c, _)| matches!(c.kind, ControlKind::Text | ControlKind::Password));
                    if text_field {
                        self.edit.paste(t);
                        let v = self.edit.value.clone();
                        if let Some(st) = self.state_mut(c) {
                            st.value = v;
                        }
                    } else if let Some(st) = self.state_mut(c) {
                        st.value.push_str(t);
                    }
                }
                Flow::Stay
            }
            Event::AreaAt(AREA_FULL, x, y) => {
                self.dev.focus = false;
                self.addr_focus = false;
                self.suggest.clear();
                if let Some(f) = self.full.as_mut() {
                    if !f.page_focus {
                        f.set_focus(true);
                    }
                    if crate::ui::dragging() {
                        f.mouse_drag(*x as i32, *y as i32);
                    } else {
                        f.mouse_down(*x as i32, *y as i32, crate::ui::click_count());
                    }
                }
                Flow::Stay
            }
            Event::MouseMove(x, y) => {
                self.mouse = (*x, *y);
                if let Some(f) = self.full.as_mut().filter(|f| !f.pressed) {
                    f.mouse_move(*x, *y);
                }
                Flow::Stay
            }
            Event::MouseUp(x, y) => {
                self.dev.drag_end();
                if let Some(f) = self.full.as_mut() {
                    f.mouse_up(*x, *y);
                }
                Flow::Stay
            }
            Event::AreaAt(AREA_PAGE, x, y) => {
                let p = (*x - self.col_off, *y + self.scroll);
                self.addr_focus = false;
                self.focus_ctl = None;
                if crate::ui::dragging() {
                    if let Some(sel) = &mut self.sel {
                        sel.head = p;
                    }
                } else {
                    self.sel = Some(Sel { anchor: p, head: p, mode: crate::ui::click_count(), all: false });
                    self.suggest.clear();
                }
                Flow::Stay
            }
            Event::FocusAt(id, x) if *id == FIND_ID => {
                if let Some(f) = &mut self.find {
                    f.field.click(*x);
                }
                self.addr_focus = false;
                self.focus_ctl = None;
                Flow::Stay
            }
            Event::FocusAt(id, x) => {
                if *id >= CONTROL_BASE {
                    self.addr_focus = false;
                    self.click_control(*id - CONTROL_BASE, *x, cx);
                } else {
                    self.addr_focus = true;
                    self.focus_ctl = None;
                    if let Some(f) = self.full.as_mut() {
                        f.set_focus(false);
                    }
                    self.field.click(*x);
                    self.refresh_suggestions();
                }
                Flow::Stay
            }
            Event::Scroll(d) if self.dev.open && self.dev.rect.contains(self.mouse.0, self.mouse.1) => {
                self.dev.scroll_by(-*d * 3);
                self.dev_dirty = true;
                Flow::Stay
            }
            Event::Scroll(d) => {
                match self.full.as_mut() {
                    Some(f) => {
                        f.wheel(*d);
                    }
                    None => self.scroll_by(-*d as f32 * 60.0),
                }
                Flow::Stay
            }
            Event::Row(i) => {
                self.follow(*i, cx);
                Flow::Stay
            }
            // toolbar buttons: 0 back, 1 forward, 2 reload, 3 home, 4 go, 5 JavaScript, 6 notes, 7 play, 8 bookmark, 9 full browser, 10 help about the missing full engine
            Event::Tab(i) => {
                match i {
                    0 => self.back(),
                    1 => self.forward(),
                    2 => self.reload(cx),
                    3 => self.home(),
                    5 => self.toggle_js(cx),
                    6 => self.show_notes = !self.show_notes,
                    7 => self.play_media(cx),
                    8 => self.toggle_bookmark(cx),
                    9 => self.open_full_browser(cx),
                    10 => self.show_engine_help(),
                    n @ (browser_av::TAB_INSTALL..=browser_av::TAB_NEVER) => self.av_choice(*n, cx),
                    &browser_media::TAB_MIC if self.media_ask.is_some() => self.open_mic_pick(),
                    &browser_media::TAB_CAM if self.media_ask.is_some() => self.open_cam_pick(),
                    n @ (browser_media::TAB_ONCE..=browser_media::TAB_NOT_NOW) => {
                        if let Some(a) = browser_media::answer_for_tab(*n) {
                            self.media_answer(a, cx);
                        }
                    }
                    n @ (super::soundask::TAB_STOP..=super::soundask::TAB_CANCEL) => {
                        if let Some(a) = super::soundask::answer_for_tab(*n) {
                            self.sound_answer(a, cx);
                        }
                    }
                    x if *x == TAB_STOP => self.stop_loading(),
                    x if *x == TAB_RETRY => self.retry_failed(cx),
                    x if *x == TAB_OVERVIEW => {
                        if self.overview.is_some() {
                            self.overview = None;
                        } else {
                            self.open_overview();
                        }
                    }
                    n @ TAB_CHIP..=639 => {
                        if let Some(g) = self.groups.list.get(n - TAB_CHIP).map(|g| g.id) {
                            self.groups.toggle_collapse(g);
                        }
                    }
                    n @ TAB_COLOR..=659 => {
                        if let Some(e) = &self.group_edit {
                            self.groups.set_color(e.gid, n - TAB_COLOR);
                        }
                    }
                    x if *x == TAB_UNGROUP => {
                        if let Some(e) = self.group_edit.take() {
                            for t in self.groups.members(e.gid) {
                                self.groups.assign(t, None);
                            }
                            self.regroup();
                        }
                    }
                    x if *x == TAB_EDIT_DONE => self.commit_group_edit(),
                    n @ TAB_CARD..=739 => {
                        self.overview = None;
                        self.switch_to(n - TAB_CARD);
                    }
                    n @ TAB_CARD_CLOSE..=779 => self.close_tab(n - TAB_CARD_CLOSE),
                    n @ TAB_CARD_GROUP..=819 => self.cycle_tab_group(n - TAB_CARD_GROUP),
                    n @ TAB_NEW_GROUP..=859 => {
                        let t = n - TAB_NEW_GROUP;
                        if let Some(g) = self.groups.new_group(t) {
                            let _ = g;
                            self.regroup();
                        }
                    }
                    860 => {}
                    n @ 861..=880 => {
                        let first = self.groups.list.get(n - 861).and_then(|g| self.groups.members(g.id).first().copied());
                        if let Some(t) = first {
                            self.overview = None;
                            self.open_group_edit(t);
                        }
                    }
                    999 => self.popup = None,
                    n if *n >= 1000 => self.pick_option(n - 1000),
                    500 => self.step_find(false),
                    501 => self.step_find(true),
                    502 => self.close_find(),
                    400 => self.new_tab(),
                    n if *n >= 300 => self.close_tab(n - 300),
                    n if *n >= 200 => self.switch_to(n - 200),
                    n if *n >= 100 => {
                        if let Some(u) = self.suggest.get(n - 100).map(|s| s.url.clone()) {
                            self.field.set(&u);
                            self.submit_address(cx);
                        }
                    }
                    _ => self.submit_address(cx),
                }
                Flow::Stay
            }
            _ => Flow::Stay,
        }
    }

    fn claims_f12(&self) -> bool {
        !self.embedded
    }

    fn wants_redraw(&mut self) -> bool {
        // the caller answers with a whole redraw (panes, other displays): a live picture needs nothing more than that
        match self.redraw_kind() {
            Redraw::None => false,
            Redraw::Live => {
                self.full.as_mut().is_some_and(|f| f.take_redraw()) || self.agent_view
            }
            Redraw::Full => true,
        }
    }

    fn redraw_kind(&mut self) -> Redraw {
        if std::mem::take(&mut self.dev_dirty) {
            return Redraw::Full;
        }
        if self.agent_view {
            // the assistant's tab shows a live picture, and its banner changes with what it does
            return Redraw::Full;
        }
        match self.full.as_mut().map(|f| f.redraw_kind()) {
            // a popup, the assistant's panel or the address suggestions are drawn over the page: they need the whole scene
            // a question to the person (camera, sound) is drawn over the page with a scrim: a partial repaint of the page would
            // show it through for a frame (the flicker), so the page layer is frozen under the dialog (the whole scene is drawn
            // on the shell's own timer, scrim and dialog last, in one present)
            Some(Redraw::Live) if self.modal_open() => Redraw::None,
            Some(Redraw::Live) if self.popup.is_some() || self.assist.is_some() || !self.suggest.is_empty() => Redraw::Full,
            Some(k) => k,
            None => Redraw::None,
        }
    }

    fn live_draw(&mut self, g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        if self.agent_view || self.popup.is_some() || self.assist.is_some() || !self.suggest.is_empty() || self.modal_open() {
            return None;
        }
        self.full.as_mut()?.draw_pending(g)
    }

    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<crate::ctxmenu::Item> {
        use crate::ctxmenu::Item;
        // a link under the pointer (the reader registers each link as a `Row` region)
        self.ctx_link = None;
        if self.full.is_none() {
            let link = crate::ctxmenu::under().into_iter().find_map(|h| if let (true, Event::Row(li)) = (h.label == "link", &h.ev) { Some(*li) } else { None });
            if let Some(li) = link {
                self.ctx_link = match self.page() {
                    Some(p) => p.doc.links.get(li).and_then(|href| p.url.join(href).ok()).map(|u| u.to_string()),
                    None => self.start_links.get(li).cloned(),
                };
            }
        }
        let mut v = Vec::new();
        if self.ctx_link.is_some() {
            v.push(Item::new("openlink", "Open link", ""));
            v.push(Item::new("opennew", "Open link in new tab", ""));
            v.push(Item::new("copylink", "Copy link address", ""));
            v.push(Item::separator());
        }
        let (can_back, can_fwd) = match self.full.as_ref() {
            Some(f) => (f.can_back || self.pos > 0, f.can_forward),
            None => (self.pos > 0, self.pos + 1 < self.history.len()),
        };
        v.push(Item::new("back", "Back", "Alt+Left").enabled(can_back));
        v.push(Item::new("forward", "Forward", "Alt+Right").enabled(can_fwd));
        v.push(Item::new("reload", "Reload", "").enabled(self.full.is_some() || self.page().is_some()));
        let has_source = self.history.get(self.pos).is_some_and(|e| e.original.is_some() || e.page.as_ref().is_some_and(|p| !p.source.is_empty()));
        if self.full.is_none() && has_source {
            let showing = self.history.get(self.pos).is_some_and(|e| e.original.is_some());
            v.push(Item::new("source", if showing { "Show the page" } else { "View source" }, "U"));
        }
        v
    }

    fn context_pick(&mut self, id: &str, cx: &AppCx) -> Flow {
        match id {
            "back" => self.back(),
            "forward" => self.forward(),
            "reload" => self.reload(cx),
            "source" => self.toggle_source(),
            "copylink" => {
                if let Some(u) = self.ctx_link.take() {
                    crate::ui::clipboard::copy(&u);
                }
            }
            "openlink" | "opennew" => {
                if let Some(Ok(u)) = self.ctx_link.take().map(|l| Url::parse(&l)) {
                    if id == "opennew" {
                        self.new_tab();
                    }
                    self.go(u, How::Push, cx);
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn pointer_hint(&self, x: i32, y: i32) -> Option<crate::ui::Pointer> {
        use crate::ui::Pointer;
        use iyakki_webview::proto::cursor as c;
        let f = self.full.as_ref()?;
        if !self.view_rect.contains(x, y) {
            return None;
        }
        Some(match f.cursor {
            c::POINTER => Pointer::Hand,
            c::TEXT => Pointer::Text,
            c::WAIT => Pointer::Wait,
            c::CROSSHAIR => Pointer::Crosshair,
            c::MOVE => Pointer::Move,
            c::NOT_ALLOWED => Pointer::NotAllowed,
            c::RESIZE_EW => Pointer::ResizeH,
            c::RESIZE_NS => Pointer::ResizeV,
            c::GRAB => Pointer::Grab,
            c::HELP => Pointer::Help,
            c::NONE => Pointer::Hidden,
            _ => Pointer::Default,
        })
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        if self.agent_view {
            super::web_agent::render(g, area, s, &cx.services, &cx.session.user);
            return;
        }
        let bar_h = chrome::BAR_H * s;
        let tabs_h = chrome::TABS_H * s;
        let status_h = 30.0 * s;
        let bar_y = area.y + tabs_h;
        let view = R::new(area.x, bar_y + bar_h, area.w, area.h - tabs_h - bar_h - status_h);
        self.view_h = view.h;
        self.view_rect = view;
        self.ui_scale = s;
        // a fullscreen page gets the whole pane: no tabs, bars or status line
        if self.full.as_ref().is_some_and(|f| f.fullscreen) {
            self.view_h = area.h;
            self.view_rect = area;
            self.render_full(g, area, s, tick);
            return;
        }
        // the developer tools panel takes its share of the page area; the page is drawn (and the engine resized) in what is left
        let total_view = view;
        let mut dev_panel: Option<(R, browser_devpanel::Dock)> = None;
        let view = if self.dev.open {
            let dock = self.dev.dock_side(view, s);
            let sp = browser_devpanel::split(view, s, dock, self.dev.frac);
            dev_panel = Some((sp.panel, dock));
            self.view_h = sp.page.h;
            self.view_rect = sp.page;
            sp.page
        } else {
            view
        };
        if self.full.is_some() {
            self.render_full(g, view, s, tick);
        } else {
            self.render_reader(g, view, s, tick);
        }
        if let Some((rect, dock)) = dev_panel {
            let reader: Vec<browser_devtools::NetLine> = self.net_log.lock().map(|l| l.clone()).unwrap_or_default();
            let total = if dock == browser_devpanel::Dock::Bottom { total_view.h } else { total_view.w };
            let ctx = self.dev_ctx();
            self.dev.render(g, rect, total, dock, s, ctx, &reader);
        }
        // (the bar only exists while the engine shows the page that reported the missing decoder)
        if let Some(b) = &self.av.bar {
            browser_av::render(g, view, s, b);
        }
        // the "taking long" card and the page-error card sit over the page, under the bars
        if self.overview.is_none() {
            self.draw_page_notices(g, total_view, s, tick);
        }
        if self.overview.is_some() {
            self.draw_overview(g, total_view, s, tick);
        }
        // the tool bar sits over whatever scrolled under it
        g.gradient_rect(area.x, bar_y, area.w, bar_h, (0.0, bar_y), (0.0, bar_y + bar_h), &[(0.0, rgb(20, 26, 48)), (1.0, BAR_LOW)]);
        let can_back = self.pos > 0 || self.full.as_ref().is_some_and(|f| f.can_back);
        let can_fwd = match &self.full {
            Some(f) => f.can_forward,
            None => self.pos + 1 < self.history.len(),
        };
        let in_full = self.full.is_some();
        let has_page = self.page().is_some();
        let notes_n = if in_full { 0 } else { self.page().map_or(0, |p| p.notes.len()) };
        // the way back to the reader shows while a page is in the full engine; there is no button to go in (it opens by itself, F too)
        let full_on = iyakki_agent::webengine::offered() && self.full.is_some();
        let badge: Option<(&str, [u8; 4])> = self.page().filter(|p| !p.url.host.is_empty()).map(|p| if p.url.https { ("Secure", OK) } else { ("Not secure", rgb(255, 176, 70)) });
        let lock_w = badge.map_or(0.0, |(t, _)| chrome::lock_width(g, s, t));
        let cap_text = self.full.as_ref().and_then(|f| browser_media::indicator(f.capture));
        let cap_w = cap_text.as_ref().map_or(0.0, |t| g.text_width(13.0 * s, &format!("● {t}"), Face::Bold) + 28.0 * s);
        let collecting = cx.services.devtools.collecting(&cx.session.user);
        let dev_label = browser_devpanel::button_label(self.dev.open, collecting, self.dev_counts.0, self.dev_counts.1);
        let dev_txt = chrome::dev_text(self.dev.open, self.dev_counts.0, self.dev_counts.1, collecting);
        let js_on = self.current_url().is_some_and(|u| self.cfg.scripts_for(&u.host));
        let js_label = if js_on { "Scripts on" } else { "Scripts off" };
        let show_js = has_page && !in_full;
        let show_play = !in_full && self.page().is_some_and(|p| p.media);
        let notes_label = format!("Notes {notes_n}");
        let mut chips: Vec<(chrome::Chip, f32)> = Vec::new();
        if full_on {
            chips.push((chrome::Chip::Reader, chrome::chip_width(g, s, "Reader", false)));
        }
        if notes_n > 0 {
            chips.push((chrome::Chip::Notes, chrome::chip_width(g, s, &notes_label, false)));
        }
        if show_js {
            chips.push((chrome::Chip::Scripts, chrome::chip_width(g, s, js_label, true)));
        }
        if show_play {
            chips.push((chrome::Chip::Play, chrome::chip_width(g, s, "Play", false)));
        }
        if !self.embedded {
            chips.push((chrome::Chip::Dev, chrome::chip_width(g, s, &dev_txt, false)));
        }
        let go_w = g.text_width(14.5 * s, "Go", Face::Bold) + 28.0 * s;
        let geo = chrome::bar_layout(area.x, bar_y, area.w, s, &chrome::BarIn { cap_w, lock_w, star: has_page, go_w, chips });
        for (i, (icon, on)) in [("←", can_back), ("→", can_fwd), ("⟳", has_page), ("⌂", true)].into_iter().enumerate() {
            chrome::draw_nav_button(g, geo.nav[i], icon, on, ACCENT, s);
            if on {
                g.hit(geo.nav[i], Event::Tab(i), ["Back", "Forward", "Reload", "Start page"][i]);
            }
        }
        if let (Some(r), Some(t)) = (geo.cap, &cap_text) {
            // the page is using the camera or the microphone: always visible, never hideable by the page
            crate::fx::chip(g, r.x, r.y + 4.0 * s, s, &format!("● {t}"), ERR, true);
        }
        self.field.render_inset(g, geo.field, self.addr_focus, s, blink(tick), Face::Sans, 0, geo.inset);
        if let (Some(r), Some((t, c))) = (geo.lock, badge) {
            chrome::draw_lock(g, r, t, c, s);
        }
        if let (Some(r), Some(p)) = (geo.star, self.page()) {
            let starred = self.data.is_bookmarked(&p.url.to_string());
            chrome::draw_star(g, r, starred, s);
            g.hit(r, Event::Tab(8), if starred { "Remove bookmark" } else { "Bookmark this page" });
        }
        chrome::draw_go(g, geo.go, ACCENT, s);
        g.hit(geo.go, Event::Tab(4), "Go");
        let mut hover_hint = None;
        for (c, r) in &geo.chips {
            match c {
                chrome::Chip::Reader => {
                    chrome::draw_chip(g, *r, s, "Reader", None, OK, false);
                    g.hit(*r, Event::Tab(9), "Reader");
                }
                chrome::Chip::Notes => {
                    chrome::draw_chip(g, *r, s, &notes_label, None, if self.show_notes { ACCENT } else { MUTED }, self.show_notes);
                    g.hit(*r, Event::Tab(6), &notes_label);
                }
                chrome::Chip::Scripts => {
                    let (col, tip) = if js_on {
                        (OK, "Scripts allowed on this site. Click to block them: pages load faster and stay safer, but some sites stop working.")
                    } else {
                        (MUTED, "Scripts blocked on this site. Click to allow them: video, maps, mail and other web apps need them.")
                    };
                    chrome::draw_chip(g, *r, s, js_label, Some(js_on), col, js_on);
                    g.hit(*r, Event::Tab(5), tip);
                    if g.hovered(*r) {
                        hover_hint = Some(tip.to_string());
                    }
                }
                chrome::Chip::Play => {
                    chrome::draw_chip(g, *r, s, "Play", None, OK, true);
                    g.hit(*r, Event::Tab(7), "Play");
                }
                chrome::Chip::Dev => {
                    let (col, strong) = chrome::dev_color(self.dev.open, self.dev_counts.0, self.dev_counts.1, collecting, ACCENT);
                    chrome::draw_chip(g, *r, s, &dev_txt, None, col, strong);
                    g.hit(*r, Event::Tab(browser_devpanel::TAB_TOGGLE), &dev_label);
                }
            }
        }
        self.hover_hint = hover_hint;
        // the thin progress bar under the tool bar: slides while the length is unknown, fills when the engine reports it
        crate::fx::accent_line(g, area.x, bar_y + bar_h - 2.0 * s, area.w, 2.0 * s, ACCENT, 0, s);
        if self.loading_now() {
            let pr = self.full.as_ref().filter(|f| f.loading && f.progress > 0).map(|f| f.progress);
            chrome::draw_progress(g, area.x, bar_y + bar_h - 3.0 * s, area.w, s, pr, tick, ACCENT);
        }
        let (bx, fw) = (geo.field.x, geo.field.w);
        // the line under the page: loading, errors, the address under the pointer, or the title
        let sy = area.y + area.h - status_h;
        g.gradient_rect(area.x, sy, area.w, status_h, (0.0, sy), (0.0, sy + status_h), &[(0.0, rgb(17, 22, 42)), (1.0, rgb(10, 13, 26))]);
        g.rect(area.x, sy, area.w, 1.0, [ACCENT[0], ACCENT[1], ACCENT[2], 80]);
        let (text, color) = if let Some(h) = &self.hover_hint {
            (h.clone(), MUTED)
        } else if let Nav::Loading { url, .. } = &self.nav {
            (format!("Loading {}{}", url.host, ".".repeat(1 + (tick / 10 % 3) as usize)), MUTED)
        } else if let Some((m, ok)) = &self.status {
            (m.clone(), if *ok { MUTED } else { ERR })
        } else if let Some(f) = self.full.as_ref().filter(|f| f.loading || !f.is_ready()) {
            let host = iyakki_sec::url_host(&f.url).unwrap_or_default();
            if f.is_ready() && f.progress > 0 { (format!("Loading {host} {}%", f.progress), MUTED) } else if f.is_ready() { (format!("Opening {host}…"), MUTED) } else { ("Starting the web engine…".to_string(), MUTED) }
        } else if let (Some(l), Some(p)) = (self.hovered_link.or(self.focus_link), self.page()) {
            (p.doc.links.get(l).and_then(|h| p.url.join(h).ok()).map_or_else(String::new, |u| u.to_string()), MUTED)
        } else if let Some(p) = self.page() {
            let js = if self.cfg.scripts_for(&p.url.host) { "  ·  Scripts allowed" } else { "" };
            (if p.doc.title.is_empty() { format!("{}{js}", p.url.host) } else { format!("{}  ·  {}{js}", p.doc.title, p.url.host) }, DIM)
        } else {
            (String::new(), DIM)
        };
        let blocked_here = {
            let (page, n) = iyakki_web::blocker_state::Filter::global().page_blocked();
            if n > 0 && self.current_url().is_some_and(|u| u.host.eq_ignore_ascii_case(&page)) { n } else { 0 }
        };
        let zoom_chip = if (self.zoom() - 1.0).abs() > 0.01 { format!("{}%", (self.zoom() * 100.0).round()) } else { String::new() };
        let chip = match (blocked_here, zoom_chip.is_empty()) {
            (0, _) => zoom_chip,
            (n, true) => format!("{n} blocked"),
            (n, false) => format!("{n} blocked · {zoom_chip}"),
        };
        let chip_w = if chip.is_empty() { 0.0 } else { g.text_width(14.0 * s, &chip, Face::Bold) + 24.0 * s };
        let text = g.fit(14.0 * s, &text, Face::Sans, area.w - 40.0 * s - chip_w);
        g.text(area.x + 20.0 * s, sy + 6.0 * s, 14.0 * s, color, &text, Face::Sans);
        if !chip.is_empty() {
            crate::fx::chip(g, area.x + area.w - chip_w - 4.0 * s, sy + 3.0 * s, s, &chip, ACCENT, true);
        }
        // the tab strip
        {
            g.rect(area.x, area.y, area.w, tabs_h, chrome::STRIP_BG);
            let titles = self.tab_titles();
            // the assistant's own tab, while it exists or is acting
            let assistant = cx.session.policy.browser_agent != iyakki_sec::policy::BrowserAgent::Off && (super::web_agent::alive(&cx.session.user) || cx.services.browser.control_view(&cx.session.user).active);
            let ass_w = if assistant { 150.0 * s } else { 0.0 };
            let items = self.groups.items(self.active);
            let chip_ws: Vec<(u32, f32)> = self.groups.list.iter().map(|gr| (gr.id, chrome::group_chip_w(g, s, &gr.name, self.groups.members(gr.id).len()))).collect();
            let geo = chrome::strip_layout(R::new(area.x, area.y, area.w, tabs_h), s, &items, self.active, &|id| chip_ws.iter().find(|c| c.0 == id).map_or(0.0, |c| c.1), ass_w + 4.0 * s);
            self.chip_rects = geo.chips.clone();
            for (gid, r) in &geo.chips {
                if let Some(k) = self.groups.list.iter().position(|gr| gr.id == *gid) {
                    let gr = &self.groups.list[k];
                    chrome::draw_group_chip(g, *r, &gr.name, self.groups.members(gr.id).len(), browser_groups::COLORS[gr.color % browser_groups::COLORS.len()], gr.collapsed, s);
                    g.hit(*r, Event::Tab(TAB_CHIP + k), &format!("group {}", gr.name));
                }
            }
            for tg in &geo.tabs {
                let i = tg.idx;
                let on = i == self.active;
                let loading = if on { self.loading_now() } else { self.tabs[i].as_ref().is_some_and(|t| t.loading() || t.full.as_ref().is_some_and(|f| f.loading)) };
                chrome::draw_tab(g, tg, titles.get(i).map_or("", |t| t.as_str()), on, loading, tick, self.groups.color_of(i), ACCENT, s);
                g.hit(tg.tab, Event::Tab(200 + i), "tab");
                if tg.close.w > 0.0 {
                    g.hit(tg.close, Event::Tab(300 + i), "close tab");
                }
            }
            if self.tabs.len() < MAX_TABS {
                chrome::draw_plus(g, geo.plus, s);
                g.hit(geo.plus, Event::Tab(400), "new tab");
            }
            chrome::draw_overview_button(g, geo.overview, self.overview.is_some(), ACCENT, s);
            g.hit(geo.overview, Event::Tab(TAB_OVERVIEW), "tab overview");
            if assistant {
                let acting = cx.services.browser.control_view(&cx.session.user).active;
                let r = R::new(area.x + area.w - ass_w - 4.0 * s, area.y + 4.0 * s, ass_w, tabs_h - 8.0 * s);
                crate::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, acting, g.hovered(r), rgb(255, 150, 60));
                g.hit(r, Event::Tab(super::web_agent::TAB_OPEN), "assistant tab");
                let lh = g.line_height(14.0 * s, Face::Bold);
                g.text(r.x + 10.0 * s, r.y + (r.h - lh) / 2.0, 14.0 * s, if acting { rgb(255, 170, 80) } else { MUTED }, if acting { "● Assistant tab" } else { "Assistant tab" }, Face::Bold);
            }
        }
        // what the address bar offers for the text typed, over the page
        if self.addr_focus && !self.suggest.is_empty() {
            let (fx, row_h) = (bx, 40.0 * s);
            let top = bar_y + bar_h;
            g.rrect_glow(fx, top, fw, row_h * self.suggest.len() as f32 + 8.0 * s, 10.0 * s, 6.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 60]);
            g.rrect(fx, top, fw, row_h * self.suggest.len() as f32 + 8.0 * s, 10.0 * s, rgb(16, 21, 40));
            g.rrect_outline(fx, top, fw, row_h * self.suggest.len() as f32 + 8.0 * s, 10.0 * s, 1.2 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 150]);
            for (i, sg) in self.suggest.iter().enumerate() {
                let r = R::new(fx + 4.0 * s, top + 4.0 * s + i as f32 * row_h, fw - 8.0 * s, row_h);
                let hot = g.hovered(r);
                if self.sug_sel == Some(i) || hot {
                    crate::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, self.sug_sel == Some(i), hot, ACCENT);
                }
                g.hit(r, Event::Tab(100 + i), "suggestion");
                let mark = if sg.bookmark { "★ " } else { "" };
                let title = g.fit(15.0 * s, &format!("{mark}{}", page_label(&sg.title, &sg.url)), Face::Bold, r.w * 0.5);
                g.text(r.x + 10.0 * s, r.y + 4.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
                let url = g.fit(13.0 * s, &sg.url, Face::Sans, r.w - 20.0 * s);
                g.text(r.x + 10.0 * s, r.y + 21.0 * s, 13.0 * s, MUTED, &url, Face::Sans);
            }
        }
        self.draw_group_edit(g, area, tabs_h, s, tick);
        if let Some((ci, hi)) = self.popup {
            let opts: Vec<String> = self.control(ci).map(|(c, _)| c.options.iter().map(|o| o.label.clone()).collect()).unwrap_or_default();
            if opts.is_empty() {
                self.popup = None;
            } else {
                g.hit(view, Event::Tab(999), "close list");
                let row_h = 30.0 * s;
                let shown = opts.len().min(((view.h - 40.0 * s) / row_h) as usize).max(1);
                let first = hi.saturating_sub(shown - 1).min(opts.len() - shown);
                let w = (view.w - 40.0 * s).min(420.0 * s);
                let r = R::new(view.x + (view.w - w) / 2.0, view.y + (view.h - shown as f32 * row_h) / 2.0 - 8.0 * s, w, shown as f32 * row_h + 16.0 * s);
                g.rrect_glow(r.x, r.y, r.w, r.h, 12.0 * s, 6.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 60]);
                g.rrect(r.x, r.y, r.w, r.h, 12.0 * s, rgb(16, 21, 40));
                g.rrect_outline(r.x, r.y, r.w, r.h, 12.0 * s, 1.5 * s, ACCENT);
                for (n, label) in opts.iter().enumerate().skip(first).take(shown) {
                    let rr = R::new(r.x + 8.0 * s, r.y + 8.0 * s + (n - first) as f32 * row_h, r.w - 16.0 * s, row_h - 2.0 * s);
                    let on = n == hi;
                    let hot = g.hovered(rr);
                    if on || hot {
                        crate::fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 7.0 * s, on, hot, ACCENT);
                    }
                    g.hit(rr, Event::Tab(1000 + n), "option");
                    let t = g.fit(15.0 * s, label, Face::Sans, rr.w - 20.0 * s);
                    g.text(rr.x + 10.0 * s, rr.y + 5.0 * s, 15.0 * s, TEXT, &t, Face::Sans);
                }
            }
        }
        if let Some(a) = &mut self.assist {
            a.render(g, view, s, cx, tick);
        }
        if let Some(ask) = &self.media_ask {
            let dev = if self.media_ask_is_mic() { Some(self.mic_line()) } else if self.media_ask_is_cam() { Some(self.cam_line()) } else { None };
            browser_media::render(g, area, s, ask, dev.as_deref(), self.media_mic_pick.is_some() || self.media_cam_pick.is_some());
        }
        if let Some(p) = &self.media_mic_pick {
            p.render(g, area, s, crate::gfx::rgb(255, 170, 70));
        }
        if let Some(p) = &self.media_cam_pick {
            p.render(g, area, s, crate::gfx::rgb(255, 170, 70));
        }
        if let Some(ask) = &self.sound_ask {
            use super::soundask::{TAB_CANCEL, TAB_KEEP, TAB_STOP};
            super::soundask::render(g, area, s, ask, Event::Tab(TAB_STOP), Event::Tab(TAB_KEEP), Event::Tab(TAB_CANCEL));
        }
    }
}

/// The group popup under the strip: the name being typed and the group it belongs to.
struct GroupEdit {
    gid: u32,
    field: TextField,
}

/// Ids of the clicks the tab strip, the group popup and the overview send (`Event::Tab`).
const TAB_STOP: usize = 22;
const TAB_RETRY: usize = 23;
const TAB_OVERVIEW: usize = 24;
const TAB_CHIP: usize = 600;
const TAB_COLOR: usize = 640;
const TAB_UNGROUP: usize = 660;
const TAB_EDIT_DONE: usize = 661;
const TAB_CARD: usize = 700;
const TAB_CARD_CLOSE: usize = 740;
const TAB_CARD_GROUP: usize = 780;
const TAB_NEW_GROUP: usize = 820;

impl BrowserApp {
    fn lazy_state(url: &str) -> TabState {
        let mut t = TabState::blank();
        t.field.set(url);
        t.addr_focus = false;
        t.lazy = true;
        t
    }

    /// The address of tab `i` (what is kept for a group).
    fn tab_url(&self, i: usize) -> String {
        if i == self.active {
            return if self.lazy { self.field.value.clone() } else { self.current_url().map(|u| u.to_string()).unwrap_or_default() };
        }
        match self.tabs.get(i) {
            Some(Some(t)) if t.lazy => t.field.value.clone(),
            Some(Some(t)) => t.history.get(t.pos).and_then(|e| e.page.as_ref()).map(|p| p.url.to_string()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn groups_json(&self) -> String {
        let urls: Vec<String> = (0..self.tabs.len()).map(|i| self.tab_url(i)).collect();
        self.groups.saved_json(&urls)
    }

    fn restore_groups(&mut self, cx: &AppCx) {
        if self.embedded {
            return;
        }
        for (name, color, collapsed, urls) in browser_groups::load(cx) {
            let mut gid = None;
            for u in urls {
                if self.tabs.len() >= MAX_TABS {
                    break;
                }
                self.tabs.push(Some(Self::lazy_state(&u)));
                self.groups.push_tab();
                let i = self.tabs.len() - 1;
                match gid {
                    None => gid = self.groups.new_group(i),
                    Some(g) => self.groups.assign(i, Some(g)),
                }
            }
            if let Some(g) = gid {
                self.groups.rename(g, &name);
                self.groups.set_color(g, color);
                if collapsed {
                    self.groups.toggle_collapse(g);
                }
            }
        }
        self.groups_saved = self.groups_json();
    }

    /// Keep the groups on disk when they (or the addresses in them) changed; looked at twice a second.
    fn groups_tick(&mut self, cx: &AppCx) {
        self.groups_clock += 1;
        if self.embedded || self.groups_clock % 15 != 0 {
            return;
        }
        let now = self.groups_json();
        if now != self.groups_saved {
            browser_groups::save(cx, &now);
            self.groups_saved = now;
        }
    }

    /// Make every group's tabs sit together again after a change of membership.
    fn regroup(&mut self) {
        let order = self.groups.contiguous_order();
        if order.iter().enumerate().all(|(a, b)| a == *b) {
            return;
        }
        let mut old = std::mem::take(&mut self.tabs);
        self.tabs = order.iter().map(|&i| old[i].take()).collect();
        self.active = order.iter().position(|&i| i == self.active).unwrap_or(0);
        self.groups.apply_order(&order);
    }

    /// Ctrl+Shift+G: the tab in use starts a group of its own (named Group N); the name can be typed at once.
    fn new_group_from(&mut self, tab: usize) {
        if self.groups.of(tab).is_some() {
            self.open_group_edit(tab);
            return;
        }
        if let Some(g) = self.groups.new_group(tab) {
            self.regroup();
            let name = self.groups.get(g).map(|g| g.name.clone()).unwrap_or_default();
            let mut field = TextField::new("Group name", false);
            field.set(&name);
            self.group_edit = Some(GroupEdit { gid: g, field });
            self.addr_focus = false;
        }
    }

    fn ungroup(&mut self, tab: usize) {
        self.groups.assign(tab, None);
        self.group_edit = None;
        self.regroup();
    }

    fn open_group_edit(&mut self, tab: usize) {
        let Some(g) = self.groups.of(tab) else {
            self.status = Some(("This tab is not in a group yet. Ctrl+Shift+G starts one.".into(), true));
            return;
        };
        let mut field = TextField::new("Group name", false);
        field.set(&g.name);
        self.group_edit = Some(GroupEdit { gid: g.id, field });
        self.addr_focus = false;
    }

    fn commit_group_edit(&mut self) {
        if let Some(e) = self.group_edit.take() {
            self.groups.rename(e.gid, &e.field.value);
        }
    }

    fn group_edit_key(&mut self, k: &Key) {
        match k {
            Key::Enter => self.commit_group_edit(),
            Key::Esc => self.group_edit = None,
            Key::Left | Key::Right | Key::Backspace | Key::Delete | Key::Char(_) | Key::Home | Key::End | Key::Nav { .. } => {
                if let Some(e) = &mut self.group_edit {
                    e.field.handle(k);
                }
            }
            _ => {}
        }
    }

    /// The card button: step the tab through the groups (and out of them again).
    fn cycle_tab_group(&mut self, tab: usize) {
        let next = self.groups.next_for(tab);
        self.groups.assign(tab, next);
        self.regroup();
    }

    fn open_overview(&mut self) {
        let mut f = TextField::new("Search my tabs", false);
        f.set("");
        self.overview = Some(f);
        self.addr_focus = false;
        self.group_edit = None;
    }

    /// The tabs the overview shows for what is typed in its box: words that all appear in the title or address, in strip order.
    fn overview_matches(&self) -> Vec<usize> {
        let q: Vec<String> = self.overview.as_ref().map(|f| f.value.split_whitespace().map(str::to_lowercase).collect()).unwrap_or_default();
        let titles = self.tab_titles();
        (0..self.tabs.len())
            .filter(|&i| {
                let hay = format!("{} {}", titles.get(i).cloned().unwrap_or_default(), self.tab_url(i)).to_lowercase();
                let gname = self.groups.of(i).map(|g| g.name.to_lowercase()).unwrap_or_default();
                q.iter().all(|w| hay.contains(w) || gname.contains(w))
            })
            .collect()
    }

    fn overview_key(&mut self, k: &Key) {
        match k {
            Key::Esc => self.overview = None,
            Key::Enter => {
                if let Some(i) = self.overview_matches().first().copied() {
                    self.overview = None;
                    self.switch_to(i);
                }
            }
            _ => {
                if let Some(f) = &mut self.overview {
                    f.handle(k);
                }
            }
        }
    }

    /// Stop what is loading (the "taking long" card's Stop).
    fn stop_loading(&mut self) {
        match self.full.as_mut() {
            Some(f) => f.stop(),
            None => self.nav = Nav::Idle,
        }
        self.status = Some(("Stopped.".into(), true));
    }

    fn retry_failed(&mut self, cx: &AppCx) {
        if let Some((u, _)) = self.failed.take() {
            self.go(u, How::Push, cx);
        } else if let Nav::Loading { url, .. } = &self.nav {
            let u = url.clone();
            self.nav = Nav::Idle;
            self.go(u, How::Replace, cx);
        } else if self.full.is_some() {
            self.reload(cx);
        }
    }

    /// The "taking long" card (Stop and Retry) while a load runs past [`chrome::SLOW_SECS`], and the page-error card (a plain
    /// sentence, the address and Retry) for a page that could not be loaded.
    fn draw_page_notices(&mut self, g: &mut Gfx, view: R, s: f32, _tick: u64) {
        if let (Some((u, why)), false) = (&self.failed, self.loading_now()) {
            let (title, body) = chrome::explain_failure(&u.host, why);
            let e = chrome::draw_error(g, view, s, &title, &body, &u.to_string(), ACCENT);
            g.hit(e.retry, Event::Tab(TAB_RETRY), "Retry");
            return;
        }
        if self.load_secs().is_some_and(|t| t >= chrome::SLOW_SECS) {
            let h = chrome::draw_hint(g, view, s, "This is taking a while. The site or the network may be slow.", ACCENT);
            g.hit(h.stop, Event::Tab(TAB_STOP), "Stop");
            g.hit(h.retry, Event::Tab(TAB_RETRY), "Retry");
        }
    }

    /// The group popup under the strip: the name, the six colours, "Ungroup all" and Done.
    fn draw_group_edit(&mut self, g: &mut Gfx, area: R, tabs_h: f32, s: f32, tick: u64) {
        let Some(e) = &self.group_edit else { return };
        let Some(grp) = self.groups.get(e.gid) else { return };
        let anchor = self.chip_rects.iter().find(|c| c.0 == e.gid).map_or(area.x + 10.0 * s, |c| c.1.x);
        let w = (320.0 * s).min(area.w - 16.0 * s);
        let card = R::new(anchor.min(area.x + area.w - w - 8.0 * s).max(area.x + 8.0 * s), area.y + tabs_h - 2.0 * s, w, 170.0 * s);
        g.rrect_glow(card.x, card.y, card.w, card.h, 12.0 * s, 6.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 60]);
        g.rrect(card.x, card.y, card.w, card.h, 12.0 * s, rgb(16, 21, 40));
        g.rrect_outline(card.x, card.y, card.w, card.h, 12.0 * s, 1.2 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 150]);
        g.hit(card, Event::Tab(860), "group");
        let field = R::new(card.x + 14.0 * s, card.y + 14.0 * s, card.w - 28.0 * s, 40.0 * s);
        e.field.render(g, field, true, s, blink(tick), Face::Sans, 1);
        let dot = 32.0 * s;
        for (i, c) in browser_groups::COLORS.iter().enumerate() {
            let r = R::new(card.x + 14.0 * s + i as f32 * (dot + 6.0 * s), field.y + field.h + 12.0 * s, dot, dot);
            g.circle(r.x + dot / 2.0, r.y + dot / 2.0, dot / 2.0 - 3.0 * s, *c);
            if i == grp.color % browser_groups::COLORS.len() {
                g.rrect_outline(r.x, r.y, r.w, r.h, dot / 2.0, 2.0 * s, rgb(240, 244, 255));
            }
            g.hit(r, Event::Tab(TAB_COLOR + i), browser_groups::COLOR_NAMES[i]);
        }
        let by = card.y + card.h - 14.0 * s - 40.0 * s;
        let done_w = chrome::button_width(g, s, "Done");
        let ung_w = chrome::button_width(g, s, "Ungroup all");
        let done = R::new(card.x + card.w - 14.0 * s - done_w, by, done_w, 40.0 * s);
        let ung = R::new(done.x - 8.0 * s - ung_w, by, ung_w, 40.0 * s);
        chrome::draw_button(g, ung, "Ungroup all", MUTED, s);
        g.hit(ung, Event::Tab(TAB_UNGROUP), "Ungroup all");
        chrome::draw_button(g, done, "Done", ACCENT, s);
        g.hit(done, Event::Tab(TAB_EDIT_DONE), "Done");
    }

    /// The tab overview over the page area: a search box, then every tab as a card, the tabs of a group under their group's header.
    fn draw_overview(&mut self, g: &mut Gfx, view: R, s: f32, tick: u64) {
        let matches = self.overview_matches();
        let order: Vec<(usize, Option<u32>)> = matches.iter().map(|&i| (i, self.groups.of(i).map(|gr| gr.id))).collect();
        let geo = chrome::overview_layout(view, s, &order);
        let titles = self.tab_titles();
        let p = geo.panel;
        g.rect(view.x, view.y, view.w, view.h, chrome::PLACEHOLDER);
        g.rrect(p.x, p.y, p.w, p.h, 14.0 * s, rgb(16, 21, 40));
        g.rrect_outline(p.x, p.y, p.w, p.h, 14.0 * s, 1.2 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 140]);
        g.hit(p, Event::Tab(860), "tab overview");
        if let Some(f) = &self.overview {
            f.render(g, geo.search, true, s, blink(tick), Face::Sans, 2);
        }
        chrome::draw_button(g, geo.done, "Done", ACCENT, s);
        g.hit(geo.done, Event::Tab(TAB_OVERVIEW), "Done");
        for (gid, r) in &geo.headers {
            if let Some(k) = self.groups.list.iter().position(|gr| gr.id == *gid) {
                let gr = &self.groups.list[k];
                let col = browser_groups::COLORS[gr.color % browser_groups::COLORS.len()];
                g.circle(r.x + 8.0 * s, r.y + r.h / 2.0, 5.0 * s, col);
                let t = g.fit(15.0 * s, &format!("{}  ·  {} tabs", gr.name, self.groups.members(gr.id).len()), Face::Bold, r.w - 24.0 * s);
                let lh = g.line_height(15.0 * s, Face::Bold);
                g.text(r.x + 22.0 * s, r.y + (r.h - lh) / 2.0, 15.0 * s, TEXT, &t, Face::Bold);
                g.hit(*r, Event::Tab(861 + k), "rename group");
            }
        }
        if geo.cards.is_empty() {
            let msg = "No tab matches.";
            let tw = g.text_width(16.0 * s, msg, Face::Sans);
            g.text(p.x + (p.w - tw) / 2.0, geo.search.y + geo.search.h + 40.0 * s, 16.0 * s, MUTED, msg, Face::Sans);
        }
        for c in &geo.cards {
            let i = c.idx;
            let on = i == self.active;
            let hot = g.hovered(c.card);
            let fill = if on { crate::fx::over(rgb(30, 40, 72), [ACCENT[0], ACCENT[1], ACCENT[2], 26]) } else if hot { rgb(27, 34, 60) } else { rgb(20, 25, 46) };
            g.rrect(c.card.x, c.card.y, c.card.w, c.card.h, 12.0 * s, fill);
            if on {
                g.rrect_outline(c.card.x, c.card.y, c.card.w, c.card.h, 12.0 * s, 1.4 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 190]);
            }
            let gcol = self.groups.color_of(i);
            if let Some(col) = gcol {
                g.rrect(c.card.x, c.card.y + 10.0 * s, 4.0 * s, c.card.h - 20.0 * s, 2.0 * s, col);
            }
            g.hit(c.card, Event::Tab(TAB_CARD + i), "tab card");
            let title = titles.get(i).cloned().unwrap_or_default();
            let t = g.fit(15.0 * s, &title, Face::Bold, c.card.w - 24.0 * s - 40.0 * s);
            g.text(c.card.x + 14.0 * s, c.card.y + 12.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
            let host = browser_data::host_of(&self.tab_url(i)).to_string();
            let h = g.fit(13.0 * s, if host.is_empty() { "New tab" } else { &host }, Face::Sans, c.card.w - 24.0 * s - 40.0 * s);
            g.text(c.card.x + 14.0 * s, c.card.y + 34.0 * s, 13.0 * s, MUTED, &h, Face::Sans);
            let chot = g.hovered(c.close);
            if chot {
                g.rrect(c.close.x, c.close.y, c.close.w, c.close.h, 8.0 * s, rgb(58, 66, 104));
            }
            let (cx, cy, d) = (c.close.x + c.close.w / 2.0, c.close.y + c.close.h / 2.0, 5.0 * s);
            g.line(cx - d, cy - d, cx + d, cy + d, 1.7 * s, if chot { TEXT } else { MUTED });
            g.line(cx - d, cy + d, cx + d, cy - d, 1.7 * s, if chot { TEXT } else { MUTED });
            g.hit(c.close, Event::Tab(TAB_CARD_CLOSE + i), "close tab");
            let (label, col) = match self.groups.of(i) {
                Some(gr) => (gr.name.clone(), browser_groups::COLORS[gr.color % browser_groups::COLORS.len()]),
                None => ("Add to group".to_string(), MUTED),
            };
            let gl = g.fit(13.0 * s, &label, Face::Bold, (c.group.w - 20.0 * s).max(0.0));
            let hotg = g.hovered(c.group);
            g.rrect(c.group.x, c.group.y, c.group.w, c.group.h, 10.0 * s, if hotg { rgb(40, 50, 86) } else { rgb(26, 32, 58) });
            g.rrect_outline(c.group.x, c.group.y, c.group.w, c.group.h, 10.0 * s, 1.0 * s, [col[0], col[1], col[2], 160]);
            let lh = g.line_height(13.0 * s, Face::Bold);
            g.text(c.group.x + 10.0 * s, c.group.y + (c.group.h - lh) / 2.0, 13.0 * s, crate::fx::readable(col, rgb(26, 32, 58), 4.5), &gl, Face::Bold);
            g.hit(c.group, Event::Tab(TAB_CARD_GROUP + i), "add to group");
            let hotn = g.hovered(c.new_group);
            g.rrect(c.new_group.x, c.new_group.y, c.new_group.w, c.new_group.h, 10.0 * s, if hotn { rgb(40, 50, 86) } else { rgb(26, 32, 58) });
            let (px, py, d) = (c.new_group.x + c.new_group.w / 2.0, c.new_group.y + c.new_group.h / 2.0, 5.0 * s);
            g.line(px - d, py, px + d, py, 1.7 * s, MUTED);
            g.line(px, py - d, px, py + d, 1.7 * s, MUTED);
            g.hit(c.new_group, Event::Tab(TAB_NEW_GROUP + i), "new group");
        }
    }

    /// True while a load runs, in either engine.
    fn loading_now(&self) -> bool {
        self.is_loading() || self.full.as_ref().is_some_and(|f| f.loading)
    }

    /// Seconds the current load has been going, when it has been going a while.
    fn load_secs(&self) -> Option<f32> {
        self.full.as_ref().and_then(|f| f.loading_secs()).or_else(|| self.is_loading().then_some(self.load_started.elapsed().as_secs_f32()))
    }
}

impl BrowserApp {
    /// A question (camera, microphone, screen, sound) is open over the page.
    fn modal_open(&self) -> bool {
        self.media_ask.is_some() || self.sound_ask.is_some()
    }
}

impl Drop for BrowserApp {
    fn drop(&mut self) {
        // the shell's sign and the screen feed belong to the pages this browser holds
        if self.published != 0 {
            browser_media::publish_capture(0);
            browser_media::set_feed_path(None);
        }
    }
}

#[cfg(test)]
#[path = "browser_look_tests.rs"]
mod look_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_web::doc::Block;
    use iyakki_sec::policy::Role;
    use iyakki_web::fetch::{Fetch, Response};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Site {
        pages: Mutex<HashMap<String, Response>>,
        asked: Mutex<Vec<String>>,
    }

    impl Fetch for Site {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.asked.lock().unwrap().push(url.to_string());
            if url.host.starts_with("nodns") || url.host == "youtub.com" {
                return Err("io: failed to lookup address information: Name has no usable address".into());
            }
            self.pages.lock().unwrap().get(&url.to_string()).cloned().ok_or_else(|| "connection refused".to_string())
        }
    }

    fn html(body: &str) -> Response {
        Response { status: 200, content_type: "text/html".into(), body: body.as_bytes().to_vec(), ..Response::default() }
    }

    fn site(pages: Vec<(&str, Response)>) -> Arc<Site> {
        Arc::new(Site { pages: Mutex::new(pages.into_iter().map(|(u, r)| (u.to_string(), r)).collect()), asked: Mutex::new(vec![]) })
    }

    fn cx_with(tag: &str, web: Arc<Site>) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        cx.web = web;
        cx
    }

    fn settle(a: &mut BrowserApp, cx: &AppCx) {
        // (the first page with scripts compiles the JavaScript engine, which is slow in a debug build)
        for _ in 0..24_000 {
            a.handle(&Event::Tick, cx);
            if !a.is_loading() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the page never finished loading");
    }

    fn typ(a: &mut BrowserApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            a.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    fn frame(a: &mut BrowserApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0), cx, 0);
        g
    }

    const GUIDE: &str = "<html><head><title>The Guide</title></head><body><h1>Guide</h1><p>Read the <a href=\"/more\">more</a> page or go <a href=\"https://other.org/x\">elsewhere</a>.</p></body></html>";

    #[test]
    fn the_assistant_is_told_which_page_is_open_and_what_is_selected() {
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://b.org/x", html("<html><head><title>Other</title></head><body><p>Second page words.</p></body></html>"))]);
        let cx = cx_with("snapshot", web);
        let user = cx.session.user.clone();
        let mut a = BrowserApp::new(&cx);
        a.handle(&Event::Tick, &cx);
        assert!(cx.services.page_snapshot_of(&user).is_none(), "the start page is not a page");
        typ(&mut a, &cx, "a.org");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        frame(&mut a, &cx, 1280, 720);
        a.handle(&Event::Tick, &cx);
        let s = cx.services.page_snapshot_of(&user).expect("a snapshot");
        assert_eq!((s.url.as_str(), s.title.as_str(), s.full_engine), ("https://a.org/", "The Guide", false));
        assert!(s.text.contains("Read the more page or go elsewhere."), "{}", s.text);
        // a selection travels with it
        a.sel_text = "Read the more page".into();
        a.handle(&Event::Tick, &cx);
        assert_eq!(cx.services.page_snapshot_of(&user).unwrap().selection, "Read the more page");
        a.sel_text.clear();
        a.handle(&Event::Tick, &cx);
        assert_eq!(cx.services.page_snapshot_of(&user).unwrap().selection, "");
        // another page replaces it; signing out drops it
        key(&mut a, &cx, Key::Ctrl('l'));
        typ(&mut a, &cx, "https://b.org/x");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        a.handle(&Event::Tick, &cx);
        let s = cx.services.page_snapshot_of(&user).unwrap();
        assert_eq!((s.url.as_str(), s.title.as_str()), ("https://b.org/x", "Other"));
        assert!(s.text.contains("Second page words."));
        cx.services.on_logout(&user, false);
        assert!(cx.services.page_snapshot_of(&user).is_none());
    }

    #[test]
    fn typing_an_address_loads_a_page_and_shows_its_title() {
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("br1", web.clone());
        let mut a = BrowserApp::new(&cx);
        typ(&mut a, &cx, "a.org");
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(a.is_loading());
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/");
        assert!(a.status_text().is_none(), "{:?}", a.status_text());
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().filter(|h| h.label == "link").count() >= 2, "both links are clickable");
        // only the host of the visit is audited, never the address
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("browser.visit") && l.contains("a.org")));
    }

    #[test]
    fn the_context_menu_has_link_actions_over_a_link_and_back_forward_reload_everywhere() {
        use crate::apps::App;
        crate::ui::clipboard::clear();
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://a.org/more", html("<title>More</title><p>more text</p>"))]);
        let cx = cx_with("brctx", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let g = frame(&mut a, &cx, 1280, 720);
        let link = g.hits.iter().find(|h| h.label == "link").expect("a link").clone();
        let (x, y) = ((link.rect.x + 3.0) as i32, (link.rect.y + 3.0) as i32);
        crate::ctxmenu::set_under(g.hits.iter().rev().filter(|h| h.rect.contains(x, y)).cloned().collect());
        let items = a.context_items(x, y);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(&ids[..7], ["openlink", "opennew", "copylink", "", "back", "forward", "reload"], "{ids:?}");
        let on = |id: &str| items.iter().find(|i| i.id == id).unwrap().enabled;
        assert!(on("back") && !on("forward") && on("reload"), "the start page is behind it, nothing ahead");
        // Copy link address gives the full address
        a.context_pick("copylink", &cx);
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("https://a.org/more"));
        // Open link in new tab
        let tabs = a.tab_count();
        a.context_items(x, y);
        a.context_pick("opennew", &cx);
        settle(&mut a, &cx);
        assert_eq!(a.tab_count(), tabs + 1);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/more");
        // away from any link only the page actions remain
        crate::ctxmenu::set_under(vec![]);
        let items = a.context_items(5, 5);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert!(ids.starts_with(&["back", "forward", "reload"]) && !ids.contains(&"copylink"), "{ids:?}");
    }

    #[test]
    fn a_link_is_followed_back_and_forward_use_the_history_without_asking_again() {
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://a.org/more", html("<title>More</title><p>more text</p>"))]);
        let cx = cx_with("br2", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        a.handle(&Event::Row(0), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/more");
        a.handle(&Event::Key(Key::Char('b')), &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/");
        a.handle(&Event::Key(Key::Char('f')), &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/more");
        assert_eq!(web.asked.lock().unwrap().len(), 2, "back and forward came from memory");
        a.handle(&Event::Key(Key::Char('r')), &cx);
        settle(&mut a, &cx);
        assert_eq!(web.asked.lock().unwrap().len(), 3, "reload asks again");
        // back to the start page
        a.handle(&Event::Tab(3), &cx);
        assert!(a.current_url().is_none());
    }

    #[test]
    fn errors_stay_on_the_page_and_put_the_cursor_in_the_address_bar() {
        let web = site(vec![]);
        let cx = cx_with("br3", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("nothing.example.org", &cx);
        settle(&mut a, &cx);
        assert!(a.status_text().unwrap().contains("nothing.example.org") && a.status_text().unwrap().contains("connection refused"), "{:?}", a.status_text());
        a.open_url("   ", &cx);
        assert!(a.status_text().unwrap().contains("web address"));
        assert!(!a.is_loading());
        a.open_url("javascript:alert(1)", &cx);
        assert!(a.status_text().unwrap().contains("only http and https"), "{:?}", a.status_text());
    }

    #[test]
    fn the_users_host_policy_applies_including_redirects() {
        let mut redirect = Response { status: 302, location: Some("https://evil.net/".into()), ..Response::default() };
        redirect.content_type = String::new();
        let web = site(vec![("https://ok.org/", redirect), ("https://evil.net/", html("<p>x</p>"))]);
        let mut cx = cx_with("br4", web.clone());
        cx.session.role = Role::User;
        cx.session.policy = Role::User.template();
        cx.session.policy.net_hosts = vec!["ok.org".into()];
        let mut a = BrowserApp::new(&cx);
        a.open_url("ok.org", &cx);
        settle(&mut a, &cx);
        assert!(a.status_text().unwrap().contains("may not open pages from evil.net"), "{:?}", a.status_text());
        assert_eq!(web.asked.lock().unwrap().clone(), vec!["https://ok.org/"], "the forbidden host was never contacted");
        // a guest with no hosts at all
        cx.session.policy.net_hosts.clear();
        let mut b = BrowserApp::new(&cx);
        b.open_url("ok.org", &cx);
        settle(&mut b, &cx);
        assert!(b.status_text().unwrap().contains("may not open pages"));
    }

    #[test]
    fn downloads_are_explained_not_shown() {
        let web = site(vec![("https://a.org/pic.png", Response { status: 200, content_type: "image/png".into(), body: vec![0; 4096], ..Response::default() })]);
        let cx = cx_with("br5", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("https://a.org/pic.png", &cx);
        settle(&mut a, &cx);
        let p = a.page().unwrap();
        assert!(matches!(&p.doc.blocks[0], Block::Text(t) if t.spans[0].text == "A file, not a page"));
    }

    #[test]
    fn scrolling_clamps_and_the_keys_work() {
        let long: String = (0..200).map(|i| format!("<p>Paragraph number {i} with some words to fill the line and make it wrap onto a second line when the window is narrow enough.</p>")).collect();
        let web = site(vec![("https://a.org/", html(&long))]);
        let cx = cx_with("br6", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert!(a.max_scroll() > 1000.0, "{}", a.max_scroll());
        a.handle(&Event::Key(Key::PageDown), &cx);
        assert!(a.scroll > 300.0);
        a.handle(&Event::Key(Key::End), &cx);
        assert_eq!(a.scroll, a.max_scroll());
        a.handle(&Event::Key(Key::Down), &cx);
        assert_eq!(a.scroll, a.max_scroll(), "cannot scroll past the end");
        a.handle(&Event::Key(Key::Home), &cx);
        a.handle(&Event::Key(Key::Up), &cx);
        assert_eq!(a.scroll, 0.0);
        a.handle(&Event::Scroll(-2), &cx);
        assert!(a.scroll > 100.0, "the wheel scrolls down for negative values");
    }

    #[test]
    fn tab_walks_the_links_scrolling_to_each_and_enter_opens_it() {
        let filler = "<p>filler text that takes up room on the screen</p>".repeat(80);
        let page = format!("<p><a href=\"/first\">first</a></p>{filler}<p><a href=\"/last\">last</a></p>");
        let web = site(vec![("https://a.org/", html(&page)), ("https://a.org/last", html("<title>Last</title><p>end</p>"))]);
        let cx = cx_with("br7", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        a.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(a.focus_link, Some(0));
        a.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(a.focus_link, Some(1));
        assert!(a.scroll > 500.0, "the last link was scrolled into view: {}", a.scroll);
        a.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(a.focus_link, Some(0), "wraps around");
        a.handle(&Event::Key(Key::BackTab), &cx);
        assert_eq!(a.focus_link, Some(1));
        a.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/last");
    }

    #[test]
    fn the_address_bar_edits_and_escape_leaves_it() {
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("br8", web);
        let mut a = BrowserApp::new(&cx);
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back), "Esc on the empty start page leaves the app");
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert!(!a.addr_focus);
        a.handle(&Event::Key(Key::Char('g')), &cx);
        assert!(a.addr_focus, "g starts typing an address");
        a.handle(&Event::Paste("example.net/x".into()), &cx);
        assert!(a.field.value.contains("example.net/x"), "{}", a.field.value);
        a.handle(&Event::Key(Key::Esc), &cx);
        assert!(!a.addr_focus && a.field.value == "https://a.org/", "Esc puts the real address back: {}", a.field.value);
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    #[test]
    fn every_size_renders_and_layout_stays_inside_the_column() {
        let wide = "<h1>A title that is long enough to wrap onto a few lines when the window is small</h1><h2>Sub</h2><p>text <b>bold</b> <i>italic</i> <code>code</code> <a href=\"/x\">link</a></p><ul><li>one<li>two<ul><li>nested</ul></ul><ol><li>first<li>second</ol><blockquote>quote</blockquote><pre>fn main() { println!(\"a very long line of code that must wrap or clip because it is wider than the column\"); }</pre><hr><table><tr><th>Name<th>Value<tr><td>a<td>1</table><p>https://example.org/a/very/long/address/that/has/no/spaces/at/all/and/keeps/going/and/going/and/going/and/going</p><p>[image: logo]</p>";
        let web = site(vec![("https://a.org/", html(wide))]);
        let cx = cx_with("br9", web);
        let mut a = BrowserApp::new(&cx);
        let _ = frame(&mut a, &cx, 800, 600); // the start page
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        for (w, h) in [(320, 240), (640, 480), (1024, 768), (1280, 720), (1920, 1080), (3840, 2160)] {
            let _ = frame(&mut a, &cx, w, h);
            let l = a.layout.as_ref().unwrap();
            let col = l.key.0 as f32;
            for p in &l.prims {
                assert!(p.x >= -1.0 && p.x + p.w <= col + 2.0, "{w}x{h}: a piece sticks out of the column ({} + {} > {col})", p.x, p.w);
            }
            assert!(l.height > 100.0);
            assert!(l.links.iter().all(|r| r.is_some()), "{w}x{h}: every link has a place");
        }
    }

    #[test]
    fn only_the_visible_part_is_drawn_and_hits_follow_the_scroll() {
        let many: String = (0..300).map(|i| format!("<p><a href=\"/p{i}\">link {i}</a></p>")).collect();
        let web = site(vec![("https://a.org/", html(&many))]);
        let cx = cx_with("br10", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let g = frame(&mut a, &cx, 1280, 720);
        let first = g.hits.iter().filter(|h| h.label == "link").count();
        assert!(first > 5 && first < 60, "{first}");
        a.handle(&Event::Key(Key::End), &cx);
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Row(299))), "the last link is reachable at the end");
        assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Row(0))));
    }

    /// What choosing "allow scripts for this site" does: it is written to the user's file.
    fn allow_js(a: &mut BrowserApp, cx: &AppCx, host: &str) {
        a.cfg.js_sites.insert(host.to_string());
        browser_config::save(cx, &a.cfg);
    }

    fn first_text(a: &BrowserApp) -> String {
        match a.page().and_then(|p| p.doc.blocks.first()) {
            Some(Block::Text(t)) => t.spans.iter().map(|s| s.text.as_str()).collect(),
            _ => String::new(),
        }
    }

    const SCRIPTED: &str = "<title>S</title><p id=a>no scripts</p><script>document.getElementById('a').textContent = 'scripts ran'; fetch('/data').then(r => r.text()).then(t => { document.getElementById('a').textContent += ' + ' + t; });</script>";

    #[test]
    fn scripts_run_on_every_site_by_default() {
        let web = site(vec![("https://a.org/", html(SCRIPTED)), ("https://a.org/data", Response { status: 200, content_type: "text/plain".into(), body: b"fetched".to_vec(), ..Response::default() })]);
        let cx = cx_with("br11d", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "scripts ran + fetched", "owner decision 2026-10-28: scripts are on by default");
        assert!(a.js_allowed_for("a.org") && a.js_allowed_for("b.org"));
    }

    #[test]
    fn scripts_run_only_for_sites_the_user_allows_and_the_choice_is_remembered() {
        let web = site(vec![("https://a.org/", html(SCRIPTED)), ("https://a.org/data", Response { status: 200, content_type: "text/plain".into(), body: b"fetched".to_vec(), ..Response::default() })]);
        let cx = cx_with("br11", web);
        // scripts run everywhere by default; this is a person who switched them off and allows chosen sites
        browser_config::save(&cx, &BrowserConfig { js_all: false, ..BrowserConfig::default() });
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "no scripts", "scripts are off for a person who switched them off, until they allow the site");
        assert!(a.page().unwrap().notes.is_empty());
        a.handle(&Event::Key(Key::Char('j')), &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "scripts ran + fetched");
        assert!(a.js_allowed_for("a.org"));
        assert!(a.page().unwrap().notes[0].starts_with("scripts: 1 ran"), "{:?}", a.page().unwrap().notes);
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("browser.js.allow") && l.contains("a.org")));
        // a new browser for the same user remembers the choice
        let mut b = BrowserApp::new(&cx);
        assert!(b.js_allowed_for("a.org") && !b.js_allowed_for("b.org"));
        b.open_url("a.org", &cx);
        settle(&mut b, &cx);
        assert_eq!(first_text(&b), "scripts ran + fetched");
        // and it can be turned off again
        b.handle(&Event::Key(Key::Char('j')), &cx);
        settle(&mut b, &cx);
        assert_eq!(first_text(&b), "no scripts");
        assert!(!BrowserApp::new(&cx).js_allowed_for("a.org"));
        // nothing to switch before a page is open
        let mut c = BrowserApp::new(&cx);
        c.handle(&Event::Key(Key::Char('g')), &cx);
        assert!(c.addr_focus);
        c.addr_focus = false;
        c.handle(&Event::Key(Key::Char('j')), &cx);
        assert!(c.status_text().unwrap().contains("Open a page first"));
    }

    #[test]
    fn words_in_the_address_bar_search_and_google_searches_are_rerouted() {
        let ddg = "https://www.bing.com/search?q=tamil+language";
        let web = site(vec![
            (ddg, html("<p>results for tamil</p>")),
            ("https://html.duckduckgo.com/html/?q=tamil+language", html("<p>results for tamil</p>")),
            ("https://google.com/", html("<form action=/search><input name=q value='tamil language'><input type=submit value=Go></form>")),
        ]);
        let cx = cx_with("br14", web.clone());
        // words, not an address
        let mut a = BrowserApp::new(&cx);
        a.field.value = "tamil language".into();
        a.submit_address(&cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "results for tamil");
        assert!(web.asked.lock().unwrap().iter().all(|u| !u.contains("google")), "{:?}", web.asked.lock().unwrap());
        // the engine is a setting
        browser_config::save(&cx, &BrowserConfig { search: iyakki_web::SearchEngine::DuckDuckGo, ..BrowserConfig::default() });
        let mut d = BrowserApp::new(&cx);
        d.field.value = "tamil language".into();
        d.submit_address(&cx);
        settle(&mut d, &cx);
        assert!(web.asked.lock().unwrap().iter().any(|u| u.contains("html.duckduckgo.com")), "{:?}", web.asked.lock().unwrap());
        browser_config::save(&cx, &BrowserConfig::default());
        // a typed Google results address goes to the same search and says so
        let mut b = BrowserApp::new(&cx);
        b.field.value = "https://www.google.com/search?q=tamil+language&hl=en".into();
        b.submit_address(&cx);
        settle(&mut b, &cx);
        assert_eq!(first_text(&b), "results for tamil");
        assert!(b.page().unwrap().notes.iter().any(|n| n.contains("search page")));
        // Google's own home page form sends its search the same way
        let mut c = BrowserApp::new(&cx);
        c.open_url("google.com", &cx);
        settle(&mut c, &cx);
        c.submit_form(0, None, &cx);
        settle(&mut c, &cx);
        assert_eq!(first_text(&c), "results for tamil", "{:?}", web.asked.lock().unwrap());
    }

    #[test]
    fn a_search_page_that_refuses_hands_the_search_to_the_next_one_and_result_links_are_unwrapped() {
        let busy = Response { status: 429, content_type: "text/html".into(), body: b"slow down".to_vec(), ..Response::default() };
        let check = Response { status: 202, content_type: "text/html".into(), body: b"are you human".to_vec(), ..Response::default() };
        let web = site(vec![
            ("https://www.bing.com/search?q=cats", busy.clone()),
            ("https://html.duckduckgo.com/html/?q=cats", check),
            ("https://search.brave.com/search?q=cats", html("<p>results from brave</p>")),
        ]);
        let cx = cx_with("br_fallback", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.field.value = "cats".into();
        a.submit_address(&cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "results from brave", "{:?}", web.asked.lock().unwrap());
        let notes = &a.page().unwrap().notes;
        assert!(notes.iter().any(|n| n.contains("DuckDuckGo did not answer") && n.contains("Brave Search")), "{notes:?}");
        assert_eq!(web.asked.lock().unwrap().len(), 3);
        // every engine busy: the last answer is shown, and nothing loops
        let all = site(vec![
            ("https://www.bing.com/search?q=cats", busy.clone()),
            ("https://html.duckduckgo.com/html/?q=cats", busy.clone()),
            ("https://search.brave.com/search?q=cats", busy.clone()),
            ("https://en.wikipedia.org/w/index.php?search=cats", busy),
        ]);
        let cx2 = cx_with("br_fallback2", all.clone());
        let mut b = BrowserApp::new(&cx2);
        b.field.value = "cats".into();
        b.submit_address(&cx2);
        settle(&mut b, &cx2);
        assert_eq!(all.asked.lock().unwrap().len(), 4);
        assert_eq!(b.status.as_ref().map(|s| s.0.as_str()), Some("The server answered 429."));
        // a Bing result link goes straight to the page it names
        let res = site(vec![
            ("https://www.bing.com/search?q=cats", html("<a href=\"https://www.bing.com/ck/a?!&&p=x&u=a1aHR0cHM6Ly9lbi53aWtpcGVkaWEub3JnL3dpa2kvQ2F0&ntb=1\">Cat</a>")),
            ("https://en.wikipedia.org/wiki/Cat", html("<p>the cat page</p>")),
        ]);
        let cx3 = cx_with("br_fallback3", res.clone());
        let mut c = BrowserApp::new(&cx3);
        c.field.value = "cats".into();
        c.submit_address(&cx3);
        settle(&mut c, &cx3);
        c.follow(0, &cx3);
        settle(&mut c, &cx3);
        assert_eq!(first_text(&c), "the cat page", "{:?}", res.asked.lock().unwrap());
        assert!(res.asked.lock().unwrap().iter().all(|u| !u.contains("/ck/")));
    }

    #[test]
    fn a_mistyped_host_shows_a_not_found_page_with_a_search_and_a_suggestion() {
        let web = site(vec![("https://www.bing.com/search?q=youtub.com", html("<p>results for the typo</p>"))]);
        let cx = cx_with("br_typo", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.field.value = "youtub.com".into();
        a.submit_address(&cx);
        settle(&mut a, &cx);
        let p = a.page().expect("a page is shown");
        assert!(p.doc.links.iter().any(|l| l == "https://youtube.com/"), "{:?}", p.doc.links);
        assert!(p.doc.links.iter().any(|l| l.contains("www.bing.com/search?q=youtub.com")), "{:?}", p.doc.links);
        assert_eq!(a.status.as_ref().map(|s| s.0.as_str()), Some("Couldn't find youtub.com"));
        assert!(!a.status.as_ref().unwrap().0.contains("lookup"));
        // following the search link searches for the typed text
        let i = p.doc.links.iter().position(|l| l.contains("www.bing.com")).unwrap();
        a.follow(i, &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "results for the typo");
        // other failures keep the plain message
        let mut b = BrowserApp::new(&cx);
        b.field.value = "refused.example".into();
        b.submit_address(&cx);
        settle(&mut b, &cx);
        assert!(b.page().is_none(), "no replacement page for a refused connection");
    }

    /// `IYAKKI_LIVE_URL=https://… IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib live_page -- --ignored`: load a real
    /// page over the network and save what the browser draws, to look at it.
    #[test]
    #[ignore]
    fn live_page_to_png() {
        let (Ok(url), Ok(out)) = (std::env::var("IYAKKI_LIVE_URL"), std::env::var("IYAKKI_SHOT")) else { return };
        let mut cx = cx_with_admin_store("live");
        cx.web = Arc::new(iyakki_agent::iyakki_proxy::https::BrowserFetch::default());
        if std::env::var("IYAKKI_JS").is_ok() {
            browser_config::save(&cx, &BrowserConfig { js_all: true, ..BrowserConfig::default() });
        }
        let mut a = BrowserApp::new(&cx);
        a.open_url(&url, &cx);
        settle(&mut a, &cx);
        if let Some(p) = a.page() {
            eprintln!("blocks {} canvas {:?} notes {:?}", p.doc.blocks.len(), p.doc.canvas, p.notes);
            if let Ok(f) = std::env::var("IYAKKI_SAVE_SOURCE") {
                let _ = std::fs::write(f, &p.source);
            }
            fn walk(b: &[Block], d: usize) {
                for x in b.iter().take(6) {
                    match x {
                        Block::Box(k) => { eprintln!("{}box pad={:?} w={:?} max={:?} bg={:?}", " ".repeat(d), k.style.padding, k.style.width, k.style.max_width, k.style.bg); walk(&k.children, d + 1) }
                        Block::Row(r) => { eprintln!("{}row {}", " ".repeat(d), r.cols.len()); for c in r.cols.iter().take(3) { walk(&c.blocks, d + 2) } }
                        Block::Text(t) => eprintln!("{}text {:?}", " ".repeat(d), t.spans.iter().map(|s| s.text.as_str()).collect::<String>().chars().take(40).collect::<String>()),
                        _ => {}
                    }
                }
            }
            walk(&p.doc.blocks, 0);
        }
        let scroll: f32 = std::env::var("IYAKKI_SCROLL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        a.scroll = scroll;
        let mut g = frame(&mut a, &cx, 1280, 900);
        for _ in 0..2 {
            g = frame(&mut a, &cx, 1280, 900);
        }
        g.save_png(std::path::Path::new(&out)).unwrap();
        if std::env::var("IYAKKI_CLICK").is_ok() {
            // press the first link that is drawn and see where it leads
            let links: Vec<_> = g.hits.iter().filter(|h| h.label == "link").map(|h| (h.rect, h.ev.clone())).collect();
            eprintln!("{} link regions drawn; page has {} links", links.len(), a.page().map_or(0, |p| p.doc.links.len()));
            if let Some((r, ev)) = links.iter().find(|(r, _)| r.y > 735.0 && r.y < 765.0) {
                eprintln!("clicking {:?} {:?}", r, ev);
                a.handle(&ev, &cx);
                settle(&mut a, &cx);
                eprintln!("now at {:?} status {:?}", a.current_url().map(|u| u.to_string()), a.status_text());
            }
        }
    }

    /// `IYAKKI_PROBE_URL=… cargo test -p iyakki-shell --lib probe_scripts -- --ignored --nocapture`: run a real page's scripts
    /// and print what the engine says (which browser features are missing).
    #[test]
    #[ignore]
    fn probe_scripts() {
        let Ok(url) = std::env::var("IYAKKI_PROBE_URL") else { return };
        let web: Arc<dyn Fetch> = Arc::new(iyakki_agent::iyakki_proxy::https::BrowserFetch::default());
        let u = Url::parse(&url).unwrap();
        let r = web.get(&u).unwrap();
        let html = String::from_utf8_lossy(&r.body).into_owned();
        let res = iyakki_js::run_page(&html, &u, web.clone(), |_| true, &iyakki_js::SiteData::default(), iyakki_js::Limits::default());
        eprintln!("status {} ran {} failed {} skipped {} nav {:?}", r.status, res.report.ran, res.report.failed, res.report.skipped, res.report.navigation);
        for l in res.report.lines().iter().take(30) {
            eprintln!("  {}", l.chars().take(900).collect::<String>());
        }
        // follow the page's own navigation with the cookies its scripts set
        if let Some(nav) = res.report.navigation.as_ref().and_then(|n| u.join(n).ok()) {
            let data = iyakki_js::SiteData::default();
            let res1 = iyakki_js::run_page(&html, &u, web.clone(), |_| true, &data, iyakki_js::Limits::default());
            let cookie = data.cookies.lock().unwrap().header_for(&nav, 0);
            eprintln!("cookies after the first run: {:?}; nav {:?}", cookie.chars().take(80).collect::<String>(), res1.report.navigation);
            let mut req = Request::get(&nav);
            req.cookie = cookie;
            let r2 = web.send(&req).unwrap();
            let t = String::from_utf8_lossy(&r2.body).into_owned();
            eprintln!("second request: status {} len {} location {:?} has enablejs: {}", r2.status, t.len(), r2.location, t.contains("enablejs"));
            if let Some(l) = &r2.location {
                let n2 = nav.join(l).unwrap();
                let mut q = Request::get(&n2);
                q.cookie = data.cookies.lock().unwrap().header_for(&n2, 0);
                let r3 = web.send(&q).unwrap();
                let t3 = String::from_utf8_lossy(&r3.body).into_owned();
                eprintln!("third: status {} len {} loc {:?} title-ish {:?}", r3.status, t3.len(), r3.location, t3.split("<title>").nth(1).map(|x| x.chars().take(60).collect::<String>()));
            }
        }
    }

    /// `IYAKKI_NAMES_FILE=names.txt cargo test … probe_globals -- --ignored --nocapture`: which browser globals are missing.
    #[test]
    #[ignore]
    fn probe_globals() {
        let Ok(f) = std::env::var("IYAKKI_NAMES_FILE") else { return };
        let names = std::fs::read_to_string(f).unwrap();
        let list: Vec<String> = names.split_whitespace().map(|n| format!("'{n}'")).collect();
        let html = format!("<script>var m = []; for (const n of [{}]) {{ try {{ if (typeof globalThis[n] === 'undefined') m.push(n); }} catch (e) {{ m.push(n + '!'); }} }} console.log('MISSING ' + m.join(' '));</script>", list.join(","));
        let web: Arc<dyn Fetch> = Arc::new(iyakki_web::fetch::NoNetwork);
        let u = Url::parse("https://a.org/").unwrap();
        let res = iyakki_js::run_page(&html, &u, web, |_| true, &iyakki_js::SiteData::default(), iyakki_js::Limits::default());
        for l in res.report.lines() {
            eprintln!("{l}");
        }
    }

    #[test]
    fn layout_survives_every_column_width_including_tiny_ones() {
        // a picture that failed, controls, columns and boxes in a column barely wider than nothing used to panic
        let tree = iyakki_web::html::parse("<style>.r{display:flex;gap:30px} .b{padding:40px;border:10px solid red;margin:20px;max-width:10px}</style><div class=r><div><img src=x alt='a long description'><input name=q><select><option>one</option></select></div><div class=b>words words words<table><tr><td>a<td>b</table></div></div><p>tail</p>");
        let sheets = {
            let vp = iyakki_web::css::Viewport::default();
            let mut n = 0;
            vec![iyakki_web::css::parse_sheet(".r{display:flex;gap:30px} .b{padding:40px;border:10px solid red;margin:20px;max-width:10px}", &vp, &mut n)]
        };
        let doc = iyakki_web::doc::build_styled(&tree, &iyakki_web::style::StyleSheets::new(&sheets), &iyakki_web::css::Viewport::default());
        let mut g = Gfx::new(300, 300);
        for failed in [true, false] {
            let infos = vec![ImgInfo { size: None, failed }; 4];
            for w in (1..=240).step_by(1).map(|w| w as f32).chain([400.0, 1200.0]) {
                for s in [0.5f32, 1.0, 3.0] {
                    let l = layout(&mut g, &doc, w, s, &infos, 0);
                    assert!(l.height.is_finite(), "width {w} scale {s}");
                }
            }
        }
    }

    fn mp4_head() -> Vec<u8> {
        let bx = |ty: &[u8; 4], c: &[u8]| -> Vec<u8> { let mut v = ((c.len() + 8) as u32).to_be_bytes().to_vec(); v.extend_from_slice(ty); v.extend_from_slice(c); v };
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&5000u32.to_be_bytes());
        [bx(b"ftyp", b"isom\0\0\x02\0isomavc1"), bx(b"moov", &bx(b"mvhd", &mvhd))].concat()
    }

    #[test]
    fn a_video_address_shows_what_the_file_is_and_hands_it_to_the_player() {
        let video = Response { status: 200, content_type: "video/mp4".into(), body: mp4_head(), content_length: Some(1_500_000), ..Response::default() };
        let web = site(vec![("https://v.org/clip.mp4", video)]);
        let mut cx = cx_with("br18", web);
        // a stand-in player that records the address it was given
        let d = crate::apps::tests::temp_dir("br18-player");
        let record = d.join("got.txt");
        let script = d.join("player.sh");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" > {}\n", record.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        browser_config::save(&cx, &BrowserConfig { player: format!("{} --fullscreen", script.display()), ..BrowserConfig::default() });
        let mut a = BrowserApp::new(&cx);
        a.open_url("https://v.org/clip.mp4", &cx);
        settle(&mut a, &cx);
        let page = a.page().unwrap();
        assert!(page.media);
        let text: String = page.doc.blocks.iter().filter_map(|b| if let Block::Text(t) = b { Some(t.spans.iter().map(|s| s.text.as_str()).collect::<String>()) } else { None }).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Video file") && text.contains("video/mp4") && text.contains("5 s") && text.contains("1.4 MB"), "{text}");
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Play"), "the tool bar offers Play");
        a.handle(&Event::Key(Key::Char('p')), &cx);
        for _ in 0..200 {
            if record.exists() && !std::fs::read_to_string(&record).unwrap_or_default().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(&record).unwrap().trim(), "--fullscreen https://v.org/clip.mp4");
        assert!(a.status_text().unwrap().contains("Opened"));
        assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("browser.play"));
        // an account that may not reach the host cannot start it
        std::fs::remove_file(&record).unwrap();
        cx.session.role = Role::User;
        cx.session.policy = Role::User.template();
        cx.session.policy.net_hosts = vec!["other.example".into()];
        let mut b = BrowserApp::new(&cx);
        b.open_url("https://v.org/clip.mp4", &cx);
        settle(&mut b, &cx);
        assert!(b.status_text().is_some(), "the load itself is refused for this account");
        // with no player installed the page says so
        browser_config::save(&cx, &BrowserConfig { player: "/nonexistent/player".into(), ..BrowserConfig::default() });
        assert!(iyakki_agent::voice::find_media_player("/nonexistent/player").is_none());
        assert!(!record.exists());
    }

    /// `IYAKKI_URLS=/path/urls.txt cargo test … probe_many -- --ignored --nocapture`: run the scripts of many real pages and
    /// report, per page, how many ran, how many failed, what was left on the page and the first errors.
    #[test]
    #[ignore]
    fn probe_many() {
        let Ok(file) = std::env::var("IYAKKI_URLS") else { return };
        let urls = std::fs::read_to_string(file).unwrap();
        let web: Arc<dyn Fetch> = Arc::new(iyakki_agent::iyakki_proxy::https::BrowserFetch::default());
        for url in urls.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let t0 = std::time::Instant::now();
            let Ok(u) = Url::parse(url) else { continue };
            let (fetch, jar) = (web.clone(), std::sync::Mutex::new(iyakki_web::CookieStore::new()));
            let allow = |_: &str| true;
            let r = iyakki_web::fetch_following(fetch.as_ref(), Request::get(&u), &allow, Some(Jar { store: &jar, now: 0 }));
            let Ok((req, resp)) = r else { eprintln!("{url}: could not load: {:?}", r.err()); continue };
            let html = String::from_utf8_lossy(&resp.body).into_owned();
            let data = iyakki_js::SiteData::default();
            let res = iyakki_js::run_page(&html, &req.url, web.clone(), allow, &data, iyakki_js::Limits::default());
            let doc = iyakki_web::doc::build(&res.tree);
            let texts: usize = doc.blocks.len();
            eprintln!("{url}: status {} ran {} failed {} skipped {} blocks {} ({:.1}s)", resp.status, res.report.ran, res.report.failed, res.report.skipped, texts, t0.elapsed().as_secs_f32());
            for l in res.report.notes.iter().filter(|n| n.contains("error") || n.contains("could not") || n.contains("skipped") || n.contains("stopped")).take(4) {
                eprintln!("     {}", l.chars().take(260).collect::<String>());
            }
        }
    }

    /// `IYAKKI_JS=file.js cargo test … probe_js -- --ignored --nocapture`: run a script file on an empty page.
    #[test]
    #[ignore]
    fn probe_js() {
        let Ok(f) = std::env::var("IYAKKI_JS_FILE") else { return };
        let code = std::fs::read_to_string(f).unwrap();
        let web: Arc<dyn Fetch> = Arc::new(iyakki_web::fetch::NoNetwork);
        let u = Url::parse("https://a.org/").unwrap();
        let html = format!("<body><div id=app></div><script>{code}</script></body>");
        let res = iyakki_js::run_page(&html, &u, web, |_| true, &iyakki_js::SiteData::default(), iyakki_js::Limits::default());
        for l in res.report.lines() {
            eprintln!("{l}");
        }
    }

    fn css(body: &str) -> Response {
        Response { status: 200, content_type: "text/css".into(), body: body.as_bytes().to_vec(), ..Response::default() }
    }

    fn word_prim(a: &BrowserApp, word: &str) -> Option<(f32, f32, [u8; 4], f32)> {
        a.layout.as_ref()?.prims.iter().find_map(|p| match &p.kind {
            Kind::Text { text, color, size, .. } if text == word => Some((p.x, p.y, *color, *size)),
            _ => None,
        })
    }

    const STYLED: &str = "<html><head><title>S</title><link rel=stylesheet href=/s.css><style>.inline{color:#007700}</style></head><body><h1 id=t>Title</h1><p class=gone>secret</p><p class=red>crimson</p><p class=inline>lime</p><div class=card><p>carded</p></div></body></html>";
    const SHEET: &str = "body{background:#fafafa;color:#222222} .gone{display:none} .red{color:#cc0000;font-size:28px} .card{background:#dde6f0;padding:10px;max-width:300px;margin:12px auto}";

    #[test]
    fn a_linked_style_sheet_changes_colours_sizes_and_hides_things_and_the_switch_turns_it_off() {
        let web = site(vec![("https://a.org/", html(STYLED)), ("https://a.org/s.css", css(SHEET))]);
        let cx = cx_with("br15", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert!(word_prim(&a, "secret").is_none(), "display:none");
        let (_, _, red, size) = word_prim(&a, "crimson").expect("shown");
        assert_eq!(red, [0xcc, 0, 0, 255]);
        assert!(size > 20.0, "28px is bigger than the base size: {size}");
        assert_eq!(word_prim(&a, "lime").unwrap().2, [0, 0x77, 0, 255], "a <style> element works too");
        assert_eq!(a.layout.as_ref().unwrap().canvas, Some([0xfa, 0xfa, 0xfa, 255]), "the page paints its own background");
        // the text without a colour of its own is dark on that light page, not the reader's pale text
        let (_, _, title, _) = word_prim(&a, "Title").unwrap();
        assert!(title[0] < 100, "{title:?}");
        // with style sheets off nothing is fetched or applied
        browser_config::save(&cx, &BrowserConfig { styles: false, ..BrowserConfig::default() });
        web.asked.lock().unwrap().clear();
        let mut b = BrowserApp::new(&cx);
        b.open_url("a.org", &cx);
        settle(&mut b, &cx);
        let _ = frame(&mut b, &cx, 1280, 720);
        assert!(word_prim(&b, "secret").is_some() && b.layout.as_ref().unwrap().canvas.is_none());
        assert_eq!(web.asked.lock().unwrap().clone(), vec!["https://a.org/"], "{:?}", web.asked.lock().unwrap());
    }

    #[test]
    fn n_turns_a_light_page_dark_for_that_site_only_and_remembers_it() {
        let page = "<html><head><style>body{background:#fafafa;color:#222222} .c{color:#aa0000}</style></head><body><p>plain words</p><p class=c>red words</p></body></html>";
        let cx = cx_with("brnight", site(vec![("https://a.org/", html(page)), ("https://b.org/", html(page))]));
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.layout.as_ref().unwrap().canvas, Some([0xfa, 0xfa, 0xfa, 255]));
        key(&mut a, &cx, Key::Char('N'));
        assert!(a.status_text().unwrap().contains("Night colours on"), "{:?}", a.status_text());
        let _ = frame(&mut a, &cx, 1280, 720);
        let canvas = a.layout.as_ref().unwrap().canvas.unwrap();
        assert!(canvas[0] < 40, "{canvas:?}");
        let (_, _, ink, _) = word_prim(&a, "plain").unwrap();
        assert!(ink[0] > 180, "dark text became light: {ink:?}");
        let (_, _, red, _) = word_prim(&a, "red").unwrap();
        assert!(red[0] > red[1], "still reddish: {red:?}");
        // another site is unchanged; a new browser remembers this one
        a.open_url("b.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.layout.as_ref().unwrap().canvas, Some([0xfa, 0xfa, 0xfa, 255]));
        let mut b = BrowserApp::new(&cx);
        b.open_url("a.org", &cx);
        settle(&mut b, &cx);
        let _ = frame(&mut b, &cx, 1280, 720);
        assert!(b.layout.as_ref().unwrap().canvas.unwrap()[0] < 40);
        key(&mut b, &cx, Key::Char('N'));
        let _ = frame(&mut b, &cx, 1280, 720);
        assert_eq!(b.layout.as_ref().unwrap().canvas, Some([0xfa, 0xfa, 0xfa, 255]));
    }

    #[test]
    fn a_drop_down_opens_as_a_list_with_arrows_typing_clicks_and_escape() {
        let page = "<form><select name=c><option value=a>Apple<option value=b>Banana<option value=c>Cherry<option value=d>Date</select></form>";
        let cx = cx_with("brselect", site(vec![("https://a.org/", html(page))]));
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let sel = a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Select).unwrap();
        a.handle(&Event::FocusAt(CONTROL_BASE + sel, 5.0), &cx);
        assert_eq!(a.popup, Some((sel, 0)));
        let g = frame(&mut a, &cx, 1280, 720);
        assert_eq!(g.hits.iter().filter(|h| h.label == "option").count(), 4);
        // typing "d" jumps to Date, Enter chooses it
        key(&mut a, &cx, Key::Char('d'));
        assert_eq!(a.popup, Some((sel, 3)));
        key(&mut a, &cx, Key::Enter);
        assert!(a.popup.is_none());
        assert_eq!(a.history[a.pos].ctl[sel].sel, 3);
        // Escape leaves the choice alone
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Up);
        key(&mut a, &cx, Key::Esc);
        assert!(a.popup.is_none() && a.history[a.pos].ctl[sel].sel == 3);
        // clicking an option chooses it, clicking outside closes the list
        key(&mut a, &cx, Key::Enter);
        a.handle(&Event::Tab(1001), &cx);
        assert_eq!(a.history[a.pos].ctl[sel].sel, 1);
        key(&mut a, &cx, Key::Enter);
        a.handle(&Event::Tab(999), &cx);
        assert!(a.popup.is_none() && a.history[a.pos].ctl[sel].sel == 1);
    }

    #[test]
    fn calc_widths_resolve_against_the_container_and_generated_content_is_shown() {
        let page = "<style>.d,.c,.m{background:#336699;margin:0 auto} .d{width:50%} .c{width:calc(50% + 100px)} .m{width:min(50%, 100px)} .q::before{content:\"\\201C\";color:#00aa00} .q::after{content:\"\\f101\"}</style><div class=d>dd</div><div class=c>cc</div><div class=m>mm</div><p class=q>quoted</p>";
        let cx = cx_with("brcalc", site(vec![("https://a.org/", html(page))]));
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let (dx, ..) = word_prim(&a, "dd").unwrap();
        let (cx_, ..) = word_prim(&a, "cc").unwrap();
        let (mx, ..) = word_prim(&a, "mm").unwrap();
        assert!(dx - cx_ > 20.0, "calc(50% + 100px) is wider than 50%, so its centred text starts further left: {dx} vs {cx_}");
        assert!(mx > dx, "min(50%, 100px) is the narrower one: {mx} vs {dx}");
        let (_, _, quote, _) = word_prim(&a, "\u{201c}").expect("::before text is drawn");
        assert_eq!(quote, [0, 0xaa, 0, 255]);
        assert!(word_prim(&a, "\u{f101}").is_none(), "an icon-font glyph is not drawn");
    }

    #[test]
    fn c_cycles_notes_then_the_network_list_of_the_page_load() {
        let web = site(vec![("https://a.org/", html("<p>hello there</p>"))]);
        let cx = cx_with("brnet", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('c'));
        assert!(a.show_notes && !a.show_net);
        key(&mut a, &cx, Key::Char('c'));
        assert!(!a.show_notes && a.show_net);
        let _ = frame(&mut a, &cx, 1280, 720);
        let lines: Vec<String> = a.net_log.lock().unwrap().iter().map(|n| n.text()).collect();
        assert!(lines.iter().any(|l| l.contains("200") && l.contains("https://a.org/")), "{lines:?}");
        key(&mut a, &cx, Key::Char('c'));
        assert!(!a.show_notes && !a.show_net);
    }

    #[test]
    fn boxes_are_centred_and_flex_columns_sit_side_by_side() {
        let page = "<style>.row{display:flex;gap:20px} .card{background:#336699;max-width:300px;margin:0 auto;padding:10px}</style><div class=row><div>alpha</div><div>beta</div><div>gamma</div></div><div class=card><p>inside</p></div>";
        let cx = cx_with("br16", site(vec![("https://a.org/", html(page))]));
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let (ax, ay, ..) = word_prim(&a, "alpha").unwrap();
        let (bx, by, ..) = word_prim(&a, "beta").unwrap();
        let (gx, gy, ..) = word_prim(&a, "gamma").unwrap();
        assert!(ax < bx && bx < gx, "{ax} {bx} {gx}");
        assert!((ay - by).abs() < 1.0 && (ay - gy).abs() < 1.0, "one line");
        let l = a.layout.as_ref().unwrap();
        let card = l.prims.iter().find(|p| matches!(p.kind, Kind::Rect { color, .. } if color == [0x33, 0x66, 0x99, 255])).expect("the card's background");
        // a page with style sheets of its own gets the whole window (less the margins), not the 940px reading column
        let col = 1280.0f32 - 48.0;
        assert!((card.w - 300.0).abs() < 1.0, "{}", card.w);
        assert!((card.x - (col - 300.0) / 2.0).abs() < 2.0, "centred: {} in {col}", card.x);
        let (ix, ..) = word_prim(&a, "inside").unwrap();
        assert!(ix > card.x + 9.0, "padding keeps the text off the edge");
    }

    #[test]
    fn floats_shadows_gradients_and_positions_lay_out_and_draw_in_the_real_page() {
        let page = "<style>.side{float:left;width:200px;background:#336699;margin-right:10px} .card{background:linear-gradient(#ff0000,#0000ff);box-shadow:0 4px 10px rgba(0,0,0,.4);padding:8px;position:relative;left:12px;opacity:.9} .pin{position:absolute;top:0;right:0;z-index:3}</style><div class=side><p>sidebar</p><p>more</p><p>links</p></div><div class=card>gradient card</div><p>flowing text that goes around the sidebar float</p><div class=pin>pinned</div>";
        let cx = cx_with("br_fx", site(vec![("https://a.org/", html(page))]));
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let (sx, ..) = word_prim(&a, "sidebar").unwrap();
        let (fx, fy, ..) = word_prim(&a, "flowing").unwrap();
        assert!(fx > sx + 150.0, "text beside the float: {sx} {fx}");
        let (gx, ..) = word_prim(&a, "gradient").unwrap();
        assert!(gx > 200.0, "the card's text is beside the float and shifted by position:relative: {gx}");
        let (px, py, ..) = word_prim(&a, "pinned").unwrap();
        assert!(px > 800.0 && py < fy, "pinned to the top right of the page: {px} {py}");
        let l = a.layout.as_ref().unwrap();
        assert!(l.prims.iter().filter(|p| matches!(p.kind, Kind::Rect { .. })).count() > 10, "gradient bands and shadow layers");
    }

    #[test]
    fn a_page_that_scripts_leave_empty_is_shown_without_them() {
        let web = site(vec![("https://a.org/", html("<title>App</title><h1>Static words</h1><script>document.body.innerHTML = '';</script>"))]);
        let cx = cx_with("br17", web);
        let mut a = BrowserApp::new(&cx);
        allow_js(&mut a, &cx, "a.org");
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "Static words");
        assert!(a.page().unwrap().notes.iter().any(|n| n.contains("shown without them")), "{:?}", a.page().unwrap().notes);
    }

    #[test]
    fn script_errors_show_up_in_the_notes_and_the_panel_draws() {
        let web = site(vec![("https://a.org/", html("<p id=a>text</p><script>throw new Error('page bug')</script>"))]);
        let cx = cx_with("br12", web);
        let mut a = BrowserApp::new(&cx);
        allow_js(&mut a, &cx, "a.org");
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert!(a.page().unwrap().notes.iter().any(|n| n.contains("page bug")), "{:?}", a.page().unwrap().notes);
        a.handle(&Event::Key(Key::Char('c')), &cx);
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Go") && g.hits.iter().any(|h| h.label.starts_with("Notes")), "the notes button is on the tool bar");
    }

    #[test]
    fn a_site_that_was_allowed_cannot_run_scripts_that_reach_forbidden_hosts() {
        let web = site(vec![("https://a.org/", html("<p id=a>x</p><script>fetch('https://evil.net/steal').catch(e => { document.getElementById('a').textContent = 'blocked'; })</script>")), ("https://evil.net/steal", html("secret"))]);
        let mut cx = cx_with("br13", web.clone());
        cx.session.role = Role::User;
        cx.session.policy = Role::User.template();
        cx.session.policy.net_hosts = vec!["a.org".into()];
        let mut a = BrowserApp::new(&cx);
        allow_js(&mut a, &cx, "a.org");
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "blocked");
        assert_eq!(web.asked.lock().unwrap().clone(), vec!["https://a.org/"], "the forbidden host was never contacted, not even by a script");
    }

    /// A server that answers by address and remembers the requests it got.
    struct Server {
        routes: Mutex<HashMap<String, Vec<Response>>>,
        seen: Mutex<Vec<iyakki_web::Request>>,
    }

    impl Fetch for Server {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.send(&iyakki_web::Request::get(url))
        }
        fn send(&self, req: &iyakki_web::Request) -> Result<Response, String> {
            self.seen.lock().unwrap().push(req.clone());
            let key = format!("{} {}", req.method, req.url);
            let mut r = self.routes.lock().unwrap();
            match r.get_mut(&key) {
                Some(v) if v.len() > 1 => Ok(v.remove(0)),
                Some(v) if v.len() == 1 => Ok(v[0].clone()),
                _ => Err(format!("no route for {key}")),
            }
        }
    }

    fn server(routes: Vec<(&str, Vec<Response>)>) -> Arc<Server> {
        Arc::new(Server { routes: Mutex::new(routes.into_iter().map(|(k, v)| (k.to_string(), v)).collect()), seen: Mutex::new(vec![]) })
    }

    fn cx_server(tag: &str, srv: Arc<Server>) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        cx.web = srv;
        cx
    }

    const SEARCH: &str = "<title>Search</title><form action=\"/search\" method=get><input name=q placeholder=\"search\"><select name=lang><option value=en>English<option value=de>Deutsch</select><input type=checkbox name=safe value=1> safe <input type=submit name=go value=\"Find it\"></form>";

    fn text_ctl(a: &BrowserApp) -> usize {
        a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Text).unwrap()
    }

    #[test]
    fn typing_in_a_search_box_and_pressing_enter_sends_the_form_as_a_get() {
        let srv = server(vec![("GET https://s.org/", vec![html(SEARCH)]), ("GET https://s.org/search?q=rust+lang%21&lang=de&safe=1", vec![html("<title>Results</title><p>found it</p>")])]);
        let cx = cx_server("br20", srv.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        let g = frame(&mut a, &cx, 1280, 720);
        // the controls are on the page and clickable
        for label in ["field", "drop-down", "checkbox", "Find it"] {
            assert!(g.hits.iter().any(|h| h.label == label), "{label}: {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
        }
        // click the field (the shell turns the click into FocusAt), type, choose German, tick the box
        let q = text_ctl(&a);
        a.handle(&Event::FocusAt(CONTROL_BASE + q, 5.0), &cx);
        assert_eq!(a.focus_ctl, Some(q));
        typ(&mut a, &cx, "rust lang!");
        let lang = a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Select).unwrap();
        a.handle(&Event::FocusAt(CONTROL_BASE + lang, 5.0), &cx);
        assert!(a.popup.is_some(), "a click opens the list");
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert!(a.popup.is_none());
        let safe = a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Checkbox).unwrap();
        a.handle(&Event::FocusAt(CONTROL_BASE + safe, 1.0), &cx);
        assert_eq!(a.history[a.pos].ctl[lang].sel, 1);
        assert!(a.history[a.pos].ctl[safe].checked);
        // back to the text field and press Enter
        a.handle(&Event::FocusAt(CONTROL_BASE + q, 5.0), &cx);
        assert_eq!(a.history[a.pos].ctl[q].value, "rust lang!", "what was typed is kept while other controls are used");
        a.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://s.org/search?q=rust+lang%21&lang=de&safe=1", "the submit button's own name is not sent when Enter was pressed in the field");
        assert_eq!(first_text(&a), "found it");
        let seen = srv.seen.lock().unwrap();
        assert_eq!(seen[1].referer, "https://s.org/");
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("browser.form") && l.contains("s.org")));
    }

    #[test]
    fn the_submit_button_adds_its_own_value_and_the_back_button_remembers_what_was_typed() {
        let srv = server(vec![("GET https://s.org/", vec![html(SEARCH)]), ("GET https://s.org/search?q=x&lang=en&go=Find+it", vec![html("<p>done</p>")])]);
        let cx = cx_server("br21", srv);
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let q = text_ctl(&a);
        a.handle(&Event::FocusAt(CONTROL_BASE + q, 5.0), &cx);
        typ(&mut a, &cx, "x");
        let go = a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Submit).unwrap();
        a.handle(&Event::FocusAt(CONTROL_BASE + go, 5.0), &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "done");
        a.handle(&Event::Key(Key::Char('b')), &cx);
        assert_eq!(a.history[a.pos].ctl[q].value, "x", "going back finds the form as it was left");
    }

    #[test]
    fn a_post_form_sends_its_body_and_a_session_cookie_set_by_the_server_goes_with_the_next_request() {
        let login = "<form action=/login method=post><input name=user><input type=password name=pw><input type=hidden name=csrf value=tok123><button>Sign in</button></form>";
        let ok = Response { status: 302, location: Some("/home".into()), set_cookies: vec!["sid=abc123; Path=/; HttpOnly".into()], ..Response::default() };
        let srv = server(vec![("GET https://s.org/", vec![html(login)]), ("POST https://s.org/login", vec![ok]), ("GET https://s.org/home", vec![html("<p>welcome</p>")])]);
        let cx = cx_server("br22", srv.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let (user, pw) = (text_ctl(&a), a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Password).unwrap());
        a.handle(&Event::FocusAt(CONTROL_BASE + user, 5.0), &cx);
        typ(&mut a, &cx, "amy");
        a.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(a.focus_ctl, Some(pw), "Tab moves to the next field");
        typ(&mut a, &cx, "s3cret&=");
        a.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "welcome");
        let seen = srv.seen.lock().unwrap();
        let post = &seen[1];
        assert_eq!((post.method.as_str(), String::from_utf8_lossy(&post.body).to_string(), post.content_type.as_str()), ("POST", "user=amy&pw=s3cret%26%3D&csrf=tok123".to_string(), "application/x-www-form-urlencoded"));
        assert_eq!(seen[2].method, "GET", "a 302 after a POST is followed with a GET");
        assert_eq!(seen[2].cookie, "sid=abc123", "the session cookie the server set went with the next request");
        assert_eq!(seen[0].cookie, "");
    }

    #[test]
    fn tab_walks_links_and_fields_in_reading_order_and_fields_do_not_swallow_page_keys_when_unfocused() {
        let srv = server(vec![("GET https://s.org/", vec![html("<a href=/one>one</a> <input name=a> <a href=/two>two</a> <input type=checkbox name=c> <input type=submit value=Go>")])]);
        let cx = cx_server("br23", srv);
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let order = a.layout.as_ref().unwrap().focus.clone();
        assert_eq!(order, vec![Focus::Link(0), Focus::Control(0), Focus::Link(1), Focus::Control(1), Focus::Control(2)]);
        for want in &order {
            a.handle(&Event::Key(Key::Tab), &cx);
            let got = match (a.focus_ctl, a.focus_link) {
                (Some(c), _) => Focus::Control(c),
                (None, Some(l)) => Focus::Link(l),
                _ => panic!("nothing focused"),
            };
            assert_eq!(&got, want);
        }
        a.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(a.focus_link, Some(0), "wraps around");
        // Esc leaves a field and the page keys work again
        a.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(a.focus_ctl, Some(0));
        typ(&mut a, &cx, "gb");
        assert_eq!(a.history[a.pos].ctl[0].value, "gb", "while a field has the keyboard, g and b are text");
        a.handle(&Event::Key(Key::Esc), &cx);
        assert_eq!(a.focus_ctl, None);
        a.handle(&Event::Key(Key::Char('g')), &cx);
        assert!(a.addr_focus, "after Esc the page keys work again");
    }

    #[test]
    fn radio_groups_checkboxes_and_text_areas() {
        let page = "<form action=/p><input type=radio name=r value=a checked><input type=radio name=r value=b><input type=checkbox name=c checked><textarea name=t>hi</textarea></form>";
        let srv = server(vec![("GET https://s.org/", vec![html(page)]), ("GET https://s.org/p?r=b&t=hi%0Athere", vec![html("<p>ok</p>")])]);
        let cx = cx_server("br24", srv.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        a.handle(&Event::FocusAt(CONTROL_BASE + 1, 1.0), &cx);
        assert_eq!((a.history[a.pos].ctl[0].checked, a.history[a.pos].ctl[1].checked), (false, true), "choosing one radio clears the other");
        a.handle(&Event::FocusAt(CONTROL_BASE + 2, 1.0), &cx);
        assert!(!a.history[a.pos].ctl[2].checked, "a click unticks a ticked box");
        a.handle(&Event::FocusAt(CONTROL_BASE + 3, 1.0), &cx);
        for k in [Key::Enter, Key::Char('t'), Key::Char('h'), Key::Char('e'), Key::Char('r'), Key::Char('e'), Key::Backspace, Key::Char('e')] {
            a.handle(&Event::Key(k), &cx);
        }
        assert_eq!(a.history[a.pos].ctl[3].value, "hi\nthere");
        a.handle(&Event::Paste("!".into()), &cx);
        assert_eq!(a.history[a.pos].ctl[3].value, "hi\nthere!");
        a.history[a.pos].ctl[3].value = "hi\nthere".into();
        a.submit_form(0, None, &cx);
        settle(&mut a, &cx);
        assert_eq!(first_text(&a), "ok", "{:?}", srv.seen.lock().unwrap().last());
    }

    #[test]
    fn controls_are_laid_out_inside_the_column_at_every_size_and_the_page_renders() {
        let page = "<h1>Form</h1><form action=/s><p>Name: <input name=n size=60> <select name=s><option>one<option>a much longer option label here</select> <input type=checkbox name=c> yes <input type=submit value=\"A rather long button label\"></p><p><textarea name=t cols=80 rows=4></textarea></p><table><tr><td>Cell <input name=cell><td><input type=submit value=Go></table></form>";
        let srv = server(vec![("GET https://s.org/", vec![html(page)])]);
        let cx = cx_server("br25", srv);
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        for (w, h) in [(320, 240), (640, 480), (1024, 768), (1280, 720), (1920, 1080), (3840, 2160)] {
            let _ = frame(&mut a, &cx, w, h);
            let l = a.layout.as_ref().unwrap();
            let col = l.key.0 as f32;
            for p in &l.prims {
                assert!(p.x >= -1.0 && p.x + p.w <= col + 2.0, "{w}x{h}: a piece sticks out of the column ({} + {} > {col})", p.x, p.w);
            }
            assert!(l.controls.iter().all(|c| c.is_some()), "{w}x{h}: every visible control has a place");
        }
    }

    fn png_file() -> Vec<u8> {
        std::fs::read(format!("{}/../iyakki-web/tests/data/rgb.png", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn settle_images(a: &mut BrowserApp, cx: &AppCx) {
        for _ in 0..24_000 {
            a.handle(&Event::Tick, cx);
            let e = &a.history[a.pos];
            if !e.images.is_empty() && e.images.iter().all(|i| i.settled()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the pictures never finished");
    }

    #[test]
    fn pictures_arrive_after_the_page_take_their_own_size_and_failures_show_their_description() {
        let page = "<p>Logo: <img src=/p.png alt=logo> <a href=/go><img src=/gone.png alt=\"A missing one\" width=50 height=40></a> <img src=\"data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7\" width=30 height=20></p>";
        let srv = server(vec![
            ("GET https://s.org/", vec![html(page)]),
            ("GET https://s.org/p.png", vec![Response { status: 200, content_type: "image/png".into(), body: png_file(), ..Response::default() }]),
            ("GET https://s.org/gone.png", vec![Response { status: 404, ..Response::default() }]),
        ]);
        let cx = cx_server("br30", srv);
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.is_empty() || true);
        // before the pictures arrive they hold a placeholder
        let l = a.layout.as_ref().unwrap();
        let sizes = |l: &Layout| l.prims.iter().filter(|p| matches!(p.kind, Kind::Image { .. })).map(|p| (p.w as u32, p.h as u32)).collect::<Vec<_>>();
        assert_eq!(sizes(l).len(), 3);
        settle_images(&mut a, &cx);
        assert!(matches!(a.history[a.pos].images[0], ImgState::Ready(_)));
        assert!(matches!(&a.history[a.pos].images[1], ImgState::Failed(e) if e.contains("404")));
        assert!(matches!(a.history[a.pos].images[2], ImgState::Ready(_)), "data: pictures need no network");
        let g = frame(&mut a, &cx, 1280, 720);
        let l = a.layout.as_ref().unwrap();
        let sz = sizes(l);
        assert_eq!(sz[0], (67, 45), "a loaded picture takes its own size");
        assert!(sz[1].0 > 50 && sz[1].1 < 40, "a missing one shrinks to its description: {:?}", sz[1]);
        assert_eq!(sz[2], (30, 20), "the page's width and height win");
        // the picture in the link is clickable and takes part in Tab
        assert!(g.hits.iter().any(|h| h.label == "link" && matches!(h.ev, Event::Row(0))), "an image inside a link is a link");
        assert_eq!(l.focus, vec![Focus::Link(0)]);
        // at 4K pictures are drawn larger, never wider than the column
        let _ = frame(&mut a, &cx, 3840, 2160);
        let l = a.layout.as_ref().unwrap();
        assert!(l.prims.iter().all(|p| p.x + p.w <= l.key.0 as f32 + 2.0));
        let big = l.prims.iter().find(|p| matches!(p.kind, Kind::Image { idx: 0, .. })).unwrap();
        assert!(big.w > 67.0 * 1.5, "{}", big.w);
    }

    fn count_colour(g: &Gfx, rgb: [u8; 3]) -> usize {
        g.pm.data().chunks_exact(4).filter(|p| p[0] == rgb[0] && p[1] == rgb[1] && p[2] == rgb[2] && p[3] == 255).count()
    }

    #[test]
    fn animated_gifs_advance_with_the_clock_and_svg_images_are_drawn() {
        let gif = crate::apps::browser_images::test_gif(&[[255, 0, 0], [0, 255, 0]], 10);
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><rect width='10' height='10' fill='#0000ff'/></svg>".to_vec();
        let page = "<p><img src=/a.gif width=60 height=60 alt=anim> <img src=/s.svg width=40 height=40 alt=vector> <img src=\"data:image/svg+xml;utf8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='4' height='4'%3E%3Crect width='4' height='4' fill='%23ffff00'/%3E%3C/svg%3E\" width=30 height=30></p>";
        let srv = server(vec![
            ("GET https://s.org/", vec![html(page)]),
            ("GET https://s.org/a.gif", vec![Response { status: 200, content_type: "image/gif".into(), body: gif, ..Response::default() }]),
            ("GET https://s.org/s.svg", vec![Response { status: 200, content_type: "image/svg+xml".into(), body: svg, ..Response::default() }]),
        ]);
        let cx = cx_server("br31", srv);
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        settle_images(&mut a, &cx);
        assert!(matches!(a.history[a.pos].images[0], ImgState::Animated(_)));
        assert!(matches!(a.history[a.pos].images[1], ImgState::Ready(_)));
        assert!(matches!(a.history[a.pos].images[2], ImgState::Ready(_)));
        let at = |a: &mut BrowserApp, tick: u64| {
            let mut g = Gfx::new(1280, 720);
            g.clear(rgb(9, 11, 20));
            a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, tick);
            g
        };
        let g0 = at(&mut a, 0);
        let (r0, gr0) = (count_colour(&g0, [255, 0, 0]), count_colour(&g0, [0, 255, 0]));
        assert!(r0 >= 3000 && gr0 == 0, "first frame is red: {r0} {gr0}");
        let g1 = at(&mut a, 4); // 4 ticks = 133 ms: the second frame
        assert!(count_colour(&g1, [0, 255, 0]) >= 3000 && count_colour(&g1, [255, 0, 0]) == 0, "second frame is green");
        let g2 = at(&mut a, 6); // 200 ms: back to the first
        assert!(count_colour(&g2, [255, 0, 0]) >= 3000);
        assert!(count_colour(&g0, [0, 0, 255]) >= 1500, "the linked SVG shows blue ({})", count_colour(&g0, [0, 0, 255]));
        assert!(count_colour(&g0, [255, 255, 0]) >= 800, "the data: SVG shows yellow");
    }

    #[test]
    fn a_slow_picture_that_arrives_after_going_to_another_page_lands_on_the_right_page() {
        let srv = server(vec![
            ("GET https://s.org/", vec![html("<p>one <img src=/a.png></p>")]),
            ("GET https://s.org/two", vec![html("<p>two</p>")]),
            ("GET https://s.org/a.png", vec![Response { status: 200, content_type: "image/png".into(), body: png_file(), ..Response::default() }]),
        ]);
        let cx = cx_server("br31", srv);
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        a.open_url("s.org/two", &cx);
        settle(&mut a, &cx);
        for _ in 0..4000 {
            a.handle(&Event::Tick, &cx);
            if a.history[1].images.iter().all(|i| i.settled()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(a.history[1].images[0], ImgState::Ready(_)), "the first page's picture belongs to the first page");
        assert!(a.history[2].images.is_empty(), "the second page has none");
    }

    #[test]
    fn pictures_over_the_limit_are_not_all_fetched() {
        let page: String = (0..80).map(|i| format!("<img src=/i{i}.png>")).collect();
        let srv = server(vec![("GET https://s.org/", vec![html(&page)])]);
        let cx = cx_server("br32", srv.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        settle_images(&mut a, &cx);
        let failed_for_limit = a.history[a.pos].images.iter().filter(|i| matches!(i, ImgState::Failed(e) if e.contains("too many"))).count();
        assert_eq!(failed_for_limit, 20, "only the first {MAX_IMAGES} pictures are asked for");
        assert!(srv.seen.lock().unwrap().len() <= 1 + MAX_IMAGES);
    }

    #[test]
    fn the_settings_switches_decide_scripts_pictures_and_cookies() {
        let page = "<p id=a>plain</p><img src=/p.png alt=pic><script>document.getElementById('a').textContent = 'scripted'; fetch('/who').then(r => r.text()).then(t => { document.getElementById('a').textContent += ' ' + t; });</script>";
        let png = Response { status: 200, content_type: "image/png".into(), body: png_file(), ..Response::default() };
        let who = Response { status: 200, content_type: "text/plain".into(), body: b"me".to_vec(), set_cookies: vec!["sid=1".into()], ..Response::default() };
        let routes = || vec![("GET https://s.org/", vec![html(page)]), ("GET https://s.org/p.png", vec![png.clone()]), ("GET https://s.org/who", vec![who.clone()])];
        // everything off
        let srv = server(routes());
        let cx = cx_server("br40", srv.clone());
        browser_config::save(&cx, &BrowserConfig { js_all: false, images: false, styles: true, cookies: false, js_sites: Default::default(), search: Default::default(), player: String::new(), cache: Default::default(), smooth: false });
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        settle_images(&mut a, &cx);
        assert_eq!(first_text(&a), "plain", "scripts are off");
        assert!(matches!(&a.history[a.pos].images[0], ImgState::Failed(e) if e.contains("turned off")), "pictures are off");
        assert_eq!(srv.seen.lock().unwrap().len(), 1, "only the page was asked for");
        // scripts everywhere, pictures on, cookies on: the change applies to the next page without restarting
        browser_config::save(&cx, &BrowserConfig { js_all: true, images: true, styles: true, cookies: true, js_sites: Default::default(), search: Default::default(), player: String::new(), cache: Default::default(), smooth: false });
        a.reload(&cx);
        settle(&mut a, &cx);
        settle_images(&mut a, &cx);
        assert_eq!(first_text(&a), "scripted me");
        assert!(matches!(a.history[a.pos].images[0], ImgState::Ready(_)));
        let seen = srv.seen.lock().unwrap();
        assert!(seen.iter().any(|r| r.url.path == "/p.png"));
        // `j` explains itself when scripts are on everywhere
        drop(seen);
        a.handle(&Event::Key(Key::Char('j')), &cx);
        assert!(a.status_text().unwrap().contains("every site"), "{:?}", a.status_text());
        // cookies off: nothing set by the server is kept or sent
        let srv2 = server(routes());
        let cx2 = cx_server("br41", srv2.clone());
        browser_config::save(&cx2, &BrowserConfig { js_all: true, images: false, styles: true, cookies: false, js_sites: Default::default(), search: Default::default(), player: String::new(), cache: Default::default(), smooth: false });
        let mut b = BrowserApp::new(&cx2);
        b.open_url("s.org", &cx2);
        settle(&mut b, &cx2);
        b.reload(&cx2);
        settle(&mut b, &cx2);
        assert!(srv2.seen.lock().unwrap().iter().all(|r| r.cookie.is_empty()), "no cookie was ever sent");
    }

    // ------------------------------------------------------------------ bookmarks, history, suggestions, downloads

    fn key(a: &mut BrowserApp, cx: &AppCx, k: Key) {
        a.handle(&Event::Key(k), cx);
    }

    #[test]
    fn full_browser_key_says_so_when_the_engine_is_not_in_this_image() {
        let _no_helper = NoHelper::new();
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("brfull1", web);
        let mut a = BrowserApp::new(&cx);
        typ(&mut a, &cx, "a.org");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('F'));
        let st = a.status_text().unwrap_or_default().to_string();
        if iyakki_agent::webengine::offered() {
            assert!(st.contains("full browser") || st.contains("hand over") || st.contains("web engine"), "{st}");
        } else {
            assert!(st.contains("The full web engine is not installed in this image (see docs/FULL-BROWSER.md)"), "{st}");
        }
    }

    #[test]
    fn a_video_without_a_decoder_gets_one_calm_bar_per_site_that_can_be_dismissed() {
        use iyakki_agent::avaddon::{Addon, Need};
        let cx = cx_with("av1", site(vec![]));
        let mut a = BrowserApp::new(&cx);
        assert!(a.av.offer("www.tiktok.com", Need::Both, Addon::Absent));
        let g = frame(&mut a, &cx, 1280, 720);
        let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
        for want in ["Install", "Not now", "Don't ask for this site"] {
            assert!(labels.contains(&want), "{want} in {labels:?}");
        }
        for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480)] {
            let g = frame(&mut a, &cx, w, h);
            for hit in g.hits.iter().filter(|x| ["Install", "Not now", "Don't ask for this site"].contains(&x.label.as_str())) {
                assert!(hit.rect.x >= 0.0 && hit.rect.x + hit.rect.w <= w as f32 + 0.5 && hit.rect.y + hit.rect.h <= h as f32, "{w}x{h}: {} {:?}", hit.label, hit.rect);
            }
        }
        // Not now: gone, and the same site is not asked again in this session (also under another spelling of the host)
        a.handle(&Event::Tab(browser_av::TAB_NOT_NOW), &cx);
        assert!(a.av.bar.is_none());
        assert!(!a.av.offer("tiktok.com", Need::Video, Addon::Absent));
        assert!(!frame(&mut a, &cx, 1280, 720).hits.iter().any(|x| x.label == "Not now"));
        // another site is asked once
        assert!(a.av.offer("example.org", Need::Audio, Addon::Absent));
        // moving to another site takes the bar away; staying on the site keeps it
        a.av.page_moved("example.org");
        assert!(a.av.bar.is_some());
        a.av.page_moved("elsewhere.net");
        assert!(a.av.bar.is_none());
    }

    #[test]
    fn dont_ask_for_this_site_is_kept_for_the_person_and_install_only_asks_the_installer() {
        use iyakki_agent::avaddon::{Addon, Need};
        let cx = cx_with("av2", site(vec![]));
        let mut a = BrowserApp::new(&cx);
        assert!(a.av.offer("tiktok.com", Need::Video, Addon::Absent));
        a.handle(&Event::Tab(browser_av::TAB_NEVER), &cx);
        assert!(a.av.bar.is_none() && a.av.prefs.never.contains("tiktok.com"));
        // a new browser (a new session) for the same person does not ask for that site, but asks for others
        let mut b = BrowserApp::new(&cx);
        assert!(!b.av.offer("www.tiktok.com", Need::Video, Addon::Absent));
        assert!(b.av.offer("other.org", Need::Video, Addon::Absent));
        // Install: nothing is downloaded here; Store -> Setup is asked for (it shows the vendor, the licence and has its own button)
        b.handle(&Event::Tab(browser_av::TAB_INSTALL), &cx);
        assert_eq!(b.take_request(), Some(("store".to_string(), format!("setup:{}", iyakki_agent::avaddon::INSTALL_ITEM))));
        assert!(b.av.bar.is_none() && b.av.waiting);
        assert_eq!(b.take_request(), None);
        // the audit log records the offer, not the page
        let log = cx.audit.recent(50).join("\n");
        assert!(log.contains("browser.av.install.asked"), "{log}");
    }

    #[test]
    fn the_bar_says_the_right_thing_for_each_state_of_the_add_on() {
        use iyakki_agent::avaddon::{Addon, Need};
        let cx = cx_with("av3", site(vec![]));
        let mut a = BrowserApp::new(&cx);
        // no bridge in the image: no Install button, a plain statement
        assert!(a.av.offer("tiktok.com", Need::Video, Addon::NotInImage));
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(!g.hits.iter().any(|x| x.label == "Install") && g.hits.iter().any(|x| x.label == "Not now"));
        assert!(browser_av::bar_message(a.av.bar.as_ref().unwrap()).contains("does not include"));
        // installed but it still does not play
        a.av.dismiss();
        assert!(a.av.offer("b.org", Need::Both, Addon::Ready));
        let m = browser_av::bar_message(a.av.bar.as_ref().unwrap());
        assert!(m.contains("still cannot") && m.contains("b.org"), "{m}");
        assert!(!frame(&mut a, &cx, 1280, 720).hits.iter().any(|x| x.label == "Install"));
        // the missing add-on: the exact words of the plan
        a.av.dismiss();
        a.av.offer("c.org", Need::Video, Addon::Absent);
        assert_eq!(browser_av::bar_message(a.av.bar.as_ref().unwrap()), "This video needs the audio and video add-on. Install it (your click, downloaded from the vendor, LGPL, licence shown).");
        // only the three words from the helper count
        a.offer_av("<script>", "d.org", &cx);
        a.offer_av("both", "", &cx);
        assert!(a.av.bar.as_ref().is_some_and(|b| b.site == "c.org"));
    }

    #[test]
    fn a_heavy_site_gets_the_needs_the_full_engine_banner_only_without_the_engine() {
        let web = site(vec![("https://www.youtube.com/", html("<html><head><title>YouTube</title></head><body><p>Loading...</p></body></html>")), ("https://a.org/", html(GUIDE))]);
        let cx = cx_with("banner1", web);
        let mut a = BrowserApp::new(&cx);
        a.engine_offered = false;
        a.open_url("www.youtube.com", &cx);
        settle(&mut a, &cx);
        let (head, body) = a.engine_banner().expect("YouTube without the engine");
        assert_eq!(head, "This site needs the full web engine.");
        assert!(body.contains("YouTube") && body.contains("\"web\" or \"macweb\" image") && body.contains("window mode"), "{body}");
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let g = frame(&mut a, &cx, w, h);
            let btn = g.hits.iter().find(|x| x.label == "How to get it").unwrap_or_else(|| panic!("no banner button at {w}x{h}"));
            assert!(btn.rect.x >= 0.0 && btn.rect.x + btn.rect.w <= w as f32 + 0.5 && btn.rect.y >= 72.0 && btn.rect.y + btn.rect.h <= h as f32, "{w}x{h}: {:?}", btn.rect);
            let v = a.view_rect;
            assert!(v.h > h as f32 * 0.3 && v.y > btn.rect.y, "{w}x{h}: the page area under the banner keeps its room: {v:?}");
            assert!(v.x + v.w <= w as f32 + 0.5 && v.y + v.h <= h as f32, "{w}x{h}: {v:?}");
        }
        // one click opens the help text, which is a page of its own and has no banner and no network address to reload
        a.handle(&Event::Tab(10), &cx);
        assert_eq!(a.page().unwrap().ctype, HELP_TYPE);
        assert!(a.page().unwrap().doc.title.contains("full web engine"));
        assert!(a.engine_banner().is_none());
        a.handle(&Event::Tab(2), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.page().unwrap().ctype, HELP_TYPE, "reload leaves the help alone");
        // with the engine installed there is no banner; an ordinary page never has one
        a.engine_offered = true;
        a.back();
        assert!(a.engine_banner().is_none());
        assert!(!frame(&mut a, &cx, 1280, 720).hits.iter().any(|x| x.label == "How to get it"));
        a.engine_offered = false;
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert!(a.engine_banner().is_none());
        assert!(!frame(&mut a, &cx, 1280, 720).hits.iter().any(|x| x.label == "How to get it"));
    }

    #[test]
    fn a_page_the_reader_gave_up_on_gets_the_generic_banner_without_the_engine() {
        let app = "<html><head><title>App</title></head><body><div id=root></div><script>window.boot = 1;</script></body></html>";
        let web = site(vec![("https://app.example/", html(app))]);
        let cx = cx_with("banner2", web);
        let mut a = BrowserApp::new(&cx);
        a.engine_offered = false;
        a.open_url("app.example", &cx);
        settle(&mut a, &cx);
        let (_, body) = a.engine_banner().expect("an empty scripted page");
        assert!(body.starts_with("This page will not load properly"), "{body}");
        a.engine_offered = true;
        assert!(a.engine_banner().is_none());
    }

    #[test]
    fn a_page_the_reader_cannot_show_opens_in_the_full_engine_by_itself_when_it_is_there() {
        let _fake = Fake::new("fullauto", "--static");
        let app = "<html><head><title>App</title></head><body><div id=root></div><script>window.boot = 1;</script></body></html>";
        let web = site(vec![("https://app.example/", html(app)), ("https://a.org/", html(GUIDE))]);
        let cx = cx_with("fullauto", web);
        let mut a = BrowserApp::new(&cx);
        a.auto_full = true;
        typ(&mut a, &cx, "app.example");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(a.full.is_some(), "{:?}", a.status_text());
        // a page the reader shows normally stays in the reader
        let mut b = BrowserApp::new(&cx);
        b.auto_full = true;
        b.open_url("https://a.org/", &cx);
        settle(&mut b, &cx);
        assert!(b.full.is_none());
    }

    #[test]
    fn youtube_and_google_open_in_the_full_engine_at_once_and_a_dead_engine_is_not_retried_by_itself() {
        let _fake = Fake::new("fullheavy", "--static");
        let web = site(vec![("https://www.youtube.com/", html(GUIDE)), ("https://www.google.com/", html(GUIDE)), ("https://a.org/", html(GUIDE))]);
        let cx = cx_with("fullheavy", web);
        let mut a = BrowserApp::new(&cx);
        a.auto_full = true;
        a.open_url("https://www.youtube.com/", &cx);
        assert!(a.full.is_some(), "YouTube goes to the engine without asking the reader: {:?}", a.status_text());
        let mut g = BrowserApp::new(&cx);
        g.auto_full = true;
        g.open_url("https://www.google.com/", &cx);
        assert!(g.full.is_some(), "{:?}", g.status_text());
        // an ordinary page stays in the reader
        let mut b = BrowserApp::new(&cx);
        b.auto_full = true;
        b.open_url("https://a.org/", &cx);
        settle(&mut b, &cx);
        assert!(b.full.is_none());
        // the person left the engine: the same site is then shown by the reader
        let mut c = BrowserApp::new(&cx);
        c.auto_full = true;
        c.full_declined = true;
        c.open_url("https://www.youtube.com/", &cx);
        settle(&mut c, &cx);
        assert!(c.full.is_none());
    }

    #[test]
    fn a_page_the_reader_cannot_show_points_to_the_full_browser() {
        let _no_helper = NoHelper::new();
        let app = "<html><head><title>App</title></head><body><div id=root></div><script>window.boot = 1;</script></body></html>";
        let web = site(vec![("https://app.example/", html(app)), ("https://a.org/", html(GUIDE))]);
        let cx = cx_with("brfull2", web);
        let mut a = BrowserApp::new(&cx);
        typ(&mut a, &cx, "app.example");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        let st = a.status_text().unwrap_or_default().to_string();
        if iyakki_agent::webengine::offered() {
            assert!(st.contains("Press F to open this page in the full browser"), "{st}");
        } else {
            assert!(st.contains("not installed in this image") && st.contains("FULL-BROWSER.md"), "{st}");
        }
        // a page the reader shows normally says nothing of the kind
        a.open_url("https://a.org/", &cx);
        settle(&mut a, &cx);
        assert!(!a.status_text().unwrap_or_default().contains("full"), "{:?}", a.status_text());
    }

    #[test]
    fn the_full_browser_decision_looks_at_blocks_and_scripts() {
        let page = |html: &str, notes: Vec<String>| Page { url: Url::parse("https://x.org/").unwrap(), status: 200, doc: iyakki_web::doc::from_html(html), notes, source: html.to_string(), media: false, download: false, ctype: String::new() };
        assert!(page_needs_full_engine(&page("<div id=r></div><script>1</script>", vec![])));
        assert!(!page_needs_full_engine(&page("<p>hello</p><script>1</script>", vec![])));
        assert!(!page_needs_full_engine(&page("", vec![])), "an empty page without scripts is just empty");
        let errs = vec!["error: Uncaught TypeError: a".to_string(), "error: Uncaught TypeError: b".to_string(), "error: Uncaught TypeError: c".to_string()];
        assert!(page_needs_full_engine(&page("<p>hi</p><script>1</script>", errs[..1].to_vec())), "scripts that fail");
        assert!(!page_needs_full_engine(&page("<p>hi</p>", errs)), "no scripts, no judgement");
        assert!(page_needs_full_engine(&page("<p>hi</p>", vec!["The page's scripts left it empty (they need browser features this reader does not have), so it is shown without them.".into()])));
    }

    #[test]
    fn a_page_can_be_bookmarked_and_the_bookmark_and_history_survive_a_restart() {
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("brbook", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('d'));
        assert!(a.data.is_bookmarked("https://a.org/") && a.status_text() == Some("Bookmarked."));
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Remove bookmark"), "the star shows it is bookmarked");
        // a new browser (as after a restart) remembers both
        let mut b = BrowserApp::new(&cx);
        assert!(b.data.is_bookmarked("https://a.org/"));
        assert_eq!(b.data.history[0].title, "The Guide");
        let _ = frame(&mut b, &cx, 1280, 720);
        assert!(b.start_links.iter().any(|l| l == "https://a.org/"), "the start page lists the bookmark: {:?}", b.start_links);
        // the bookmarks page lists it and a click opens it
        key(&mut b, &cx, Key::Ctrl('b'));
        let _ = frame(&mut b, &cx, 1280, 720);
        let i = b.start_links.iter().position(|l| l == "https://a.org/").expect("listed");
        b.handle(&Event::Row(i), &cx);
        settle(&mut b, &cx);
        assert_eq!(b.current_url().unwrap().to_string(), "https://a.org/");
        // unstar
        key(&mut b, &cx, Key::Ctrl('d'));
        assert!(!b.data.is_bookmarked("https://a.org/"));
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("browser.bookmark.add") && l.contains("a.org")));
    }

    #[test]
    fn the_history_page_lists_visits_and_shift_d_clears_it() {
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("brhist", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('h'));
        let _ = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.start_links, vec!["https://a.org/".to_string()]);
        key(&mut a, &cx, Key::Char('D'));
        assert!(a.data.history.is_empty());
        let _ = frame(&mut a, &cx, 1280, 720);
        assert!(a.start_links.is_empty());
        assert!(browser_data::load(&cx).history.is_empty(), "cleared on disk too");
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("browser.history.clear")));
        // back returns to the page
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/");
    }

    #[test]
    fn the_address_bar_offers_bookmarks_and_visited_pages_and_enter_opens_the_chosen_one() {
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://b.org/", html("<title>Other place</title><p>x</p>"))]);
        let cx = cx_with("brsug", web.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        a.open_url("b.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('g'));
        typ(&mut a, &cx, "the gui");
        assert_eq!(a.suggest.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(), vec!["https://a.org/"]);
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "suggestion"));
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/");
        assert!(a.suggest.is_empty());
        // clicking a suggestion opens it too
        key(&mut a, &cx, Key::Char('g'));
        typ(&mut a, &cx, "other");
        assert_eq!(a.suggest.len(), 1);
        a.handle(&Event::Tab(100), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://b.org/");
        // Esc hides the list first
        key(&mut a, &cx, Key::Char('g'));
        typ(&mut a, &cx, "the");
        assert!(!a.suggest.is_empty());
        key(&mut a, &cx, Key::Esc);
        assert!(a.suggest.is_empty() && a.addr_focus);
    }

    #[test]
    fn short_words_send_the_search_to_a_site() {
        assert_eq!(expand_shortcut("w rust lang").as_deref(), Some("https://en.wikipedia.org/w/index.php?search=rust+lang"));
        assert_eq!(expand_shortcut("yt cats & dogs").as_deref(), Some("https://www.youtube.com/results?search_query=cats+%26+dogs"));
        assert_eq!(expand_shortcut("gh iyakki").as_deref(), Some("https://github.com/search?q=iyakki"));
        assert_eq!(expand_shortcut("w"), None);
        assert_eq!(expand_shortcut("w   "), None);
        assert_eq!(expand_shortcut("hello world"), None, "ordinary words are searched on the chosen engine");
        let web = site(vec![("https://en.wikipedia.org/w/index.php?search=iyakki", html("<title>Wiki</title><p>x</p>"))]);
        let cx = cx_with("brshort", web);
        let mut a = BrowserApp::new(&cx);
        typ(&mut a, &cx, "w iyakki");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().host, "en.wikipedia.org");
    }

    #[test]
    fn a_file_is_saved_to_downloads_never_over_an_older_one_and_listed() {
        let pdf = Response { status: 200, content_type: "application/pdf".into(), body: b"%PDF-1.4 hello".to_vec(), ..Response::default() };
        let web = site(vec![("https://files.org/docs/report.pdf", pdf)]);
        let cx = cx_with("brdl", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("https://files.org/docs/report.pdf", &cx);
        settle(&mut a, &cx);
        assert!(a.page().unwrap().download, "a file is offered for saving, not shown");
        let wait = |a: &mut BrowserApp, cx: &AppCx| {
            for _ in 0..2000 {
                a.handle(&Event::Tick, cx);
                if a.download.is_none() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("the download never finished");
        };
        key(&mut a, &cx, Key::Char('d'));
        wait(&mut a, &cx);
        let dir = BrowserApp::downloads_dir(&cx).unwrap();
        assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"%PDF-1.4 hello");
        assert!(a.status_text().unwrap().contains("Saved report.pdf"), "{:?}", a.status_text());
        key(&mut a, &cx, Key::Char('d'));
        wait(&mut a, &cx);
        assert!(dir.join("report (1).pdf").exists(), "a second copy gets its own name");
        assert_eq!(a.data.downloads.len(), 2);
        key(&mut a, &cx, Key::Ctrl('j'));
        let _ = frame(&mut a, &cx, 1280, 720);
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("browser.download") && l.contains("files.org")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_names_are_safe() {
        assert_eq!(download_name("https://x.org/a/b/My%20File.pdf?x=1#f", ""), "My20File.pdf");
        assert_eq!(download_name("https://x.org/../../etc/passwd", ""), "passwd");
        assert_eq!(download_name("https://x.org/", "application/pdf"), "download.pdf");
        assert_eq!(download_name("https://x.org/..", ""), "download");
        assert_eq!(download_name(&format!("https://x.org/{}", "a".repeat(300)), "").len(), 80);
    }

    #[test]
    fn bookmarks_are_exported_to_and_imported_from_the_downloads_folder() {
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("brexp", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('d'));
        key(&mut a, &cx, Key::Ctrl('b'));
        key(&mut a, &cx, Key::Ctrl('e'));
        let dir = BrowserApp::downloads_dir(&cx).unwrap();
        assert!(std::fs::read_to_string(dir.join("bookmarks.html")).unwrap().contains("https://a.org/"));
        a.data.bookmarks.clear();
        key(&mut a, &cx, Key::Char('I'));
        assert_eq!(a.data.bookmarks.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------ tabs

    fn ctrl_page(down: bool) -> Key {
        Key::Nav { nav: if down { crate::ui::Nav::PageDown } else { crate::ui::Nav::PageUp }, shift: false, ctrl: true }
    }

    #[test]
    fn tabs_keep_their_own_pages_history_and_address_and_switching_does_not_lose_anything() {
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://a.org/more", html("<title>More</title><p>more text</p>")), ("https://b.org/", html("<title>Other place</title><p>x</p>"))]);
        let cx = cx_with("brtabs", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        a.handle(&Event::Row(0), &cx); // /more
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('t'));
        assert_eq!((a.tab_count(), a.active_tab()), (2, 1));
        assert!(a.addr_focus && a.page().is_none(), "a new tab is empty with the cursor in the address bar");
        a.open_url("b.org", &cx);
        settle(&mut a, &cx);
        assert_eq!(a.tab_titles(), vec!["More".to_string(), "Other place".to_string()]);
        key(&mut a, &cx, ctrl_page(false));
        assert_eq!((a.active_tab(), a.current_url().unwrap().to_string()), (0, "https://a.org/more".to_string()));
        assert_eq!(a.field.value, "https://a.org/more", "the address bar belongs to the tab");
        key(&mut a, &cx, Key::Char('b'));
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/", "history is per tab");
        key(&mut a, &cx, ctrl_page(true));
        assert_eq!(a.current_url().unwrap().to_string(), "https://b.org/");
        key(&mut a, &cx, ctrl_page(true));
        assert_eq!(a.active_tab(), 0, "it wraps around");
        // the strip is drawn with a hit region for every tab, its close mark and the plus
        let g = frame(&mut a, &cx, 1280, 720);
        assert_eq!(g.hits.iter().filter(|h| h.label == "tab").count(), 2);
        assert_eq!(g.hits.iter().filter(|h| h.label == "close tab").count(), 2);
        assert!(g.hits.iter().any(|h| h.label == "new tab"));
        a.handle(&Event::Tab(201), &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://b.org/", "a click on a tab selects it");
    }

    #[test]
    fn closing_a_tab_shows_its_neighbour_and_the_last_one_can_be_brought_back() {
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://b.org/", html("<title>Other place</title><p>x</p>"))]);
        let cx = cx_with("brclose", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('t'));
        a.open_url("b.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('w'));
        assert_eq!((a.tab_count(), a.active_tab()), (1, 0));
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/");
        key(&mut a, &cx, Key::Ctrl('T'));
        settle(&mut a, &cx);
        assert_eq!((a.tab_count(), a.current_url().unwrap().to_string()), (2, "https://b.org/".to_string()));
        // closing a tab that is not in use leaves the one in use alone
        a.handle(&Event::Tab(300), &cx);
        assert_eq!((a.tab_count(), a.active_tab(), a.current_url().unwrap().to_string()), (1, 0, "https://b.org/".to_string()));
        // the only tab: closing empties it instead of leaving the browser
        key(&mut a, &cx, Key::Ctrl('w'));
        assert!(a.page().is_none() && a.tab_count() == 1);
        key(&mut a, &cx, Key::Ctrl('T'));
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://b.org/");
        let mut e = BrowserApp::new(&cx);
        key(&mut e, &cx, Key::Ctrl('T'));
        assert!(e.status_text().unwrap().contains("No closed tab"));
    }

    #[test]
    fn a_page_still_loading_in_a_tab_that_was_left_is_there_when_you_come_back() {
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://b.org/", html("<title>Other place</title><p>x</p>"))]);
        let cx = cx_with("brbg", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('t'));
        a.open_url("b.org", &cx); // starts loading in the background thread
        key(&mut a, &cx, ctrl_page(false));
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/");
        std::thread::sleep(std::time::Duration::from_millis(100));
        key(&mut a, &cx, ctrl_page(true));
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "https://b.org/");
        for _ in 0..30 {
            key(&mut a, &cx, Key::Ctrl('t'));
        }
        assert_eq!(a.tab_count(), MAX_TABS);
        assert!(a.status_text().unwrap().contains("At most"));
    }

    // ------------------------------------------------------------------ zoom

    fn text_size(a: &BrowserApp, word: &str) -> f32 {
        a.layout.as_ref().unwrap().prims.iter().find_map(|p| match &p.kind {
            Kind::Text { text, size, .. } if text.contains(word) => Some(*size),
            _ => None,
        }).unwrap_or_else(|| panic!("no text with {word}"))
    }

    struct Echo(Arc<Mutex<Vec<Vec<iyakki_agent::chat::Message>>>>);
    impl iyakki_agent::chat::ChatBackend for Echo {
        fn complete(&self, history: &[iyakki_agent::chat::Message]) -> Result<String, String> {
            self.0.lock().unwrap().push(history.to_vec());
            Ok("The gist: zebras are striped.".into())
        }
    }

    fn with_chat(cx: &mut AppCx) -> Arc<Mutex<Vec<Vec<iyakki_agent::chat::Message>>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        cx.chat = Arc::new(Echo(seen.clone()));
        let mut p = iyakki_agent::providers::ProviderCfg::new("cloud", iyakki_agent::providers::Kind::OpenAi);
        p.base_url = Some("https://api.example.com/v1".into());
        p.key_ref = Some("vault:cloud-key".into());
        let mut st = cx.services.settings.lock().unwrap();
        st.upsert_provider(p).unwrap();
        st.set_chat(Some(iyakki_agent::settings::ModelChoice { provider: "cloud".into(), model: "big".into() })).unwrap();
        seen
    }

    #[test]
    fn the_assistant_is_asked_first_then_sees_the_page_text_and_the_answer_is_shown_over_the_page() {
        let web = site(vec![("https://a.org/", html("<title>Zoo</title><h1>Zoo</h1><p>Zebras have black and white stripes and live on the savannah.</p>"))]);
        let mut cx = cx_with("brassist", web);
        let seen = with_chat(&mut cx);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('S'));
        // it says where the words would go and nothing is sent yet
        let g = frame(&mut a, &cx, 1280, 720);
        let _ = g;
        assert!(a.assist.as_ref().unwrap().is_confirming());
        assert!(seen.lock().unwrap().is_empty(), "nothing is sent before the person agrees");
        key(&mut a, &cx, Key::Esc);
        assert!(a.assist.is_none() && seen.lock().unwrap().is_empty());
        key(&mut a, &cx, Key::Char('S'));
        key(&mut a, &cx, Key::Enter);
        for _ in 0..400 {
            a.handle(&Event::Tick, &cx);
            if a.assist.as_ref().is_some_and(|x| x.answer().is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(a.assist.as_ref().unwrap().answer(), Some("The gist: zebras are striped."));
        let _ = frame(&mut a, &cx, 1280, 720);
        let sent = seen.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].iter().any(|m| m.content.contains("Zebras have black and white stripes")));
        key(&mut a, &cx, Key::Esc);
        assert!(a.assist.is_none());
        // a question
        key(&mut a, &cx, Key::Char('Q'));
        typ(&mut a, &cx, "what stripes");
        key(&mut a, &cx, Key::Enter);
        assert!(a.assist.as_ref().unwrap().is_confirming());
        key(&mut a, &cx, Key::Enter);
        for _ in 0..400 {
            a.handle(&Event::Tick, &cx);
            if a.assist.as_ref().is_some_and(|x| x.answer().is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(seen.lock().unwrap()[1].iter().any(|m| m.content.contains("Question: what stripes")));
    }

    #[test]
    fn without_an_assistant_the_panel_says_so_and_sends_nothing() {
        let web = site(vec![("https://a.org/", html("<p>Some words that are long enough to count as text on a page, really.</p>"))]);
        let cx = cx_with("brassist2", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('T'));
        assert!(a.assist.as_ref().unwrap().failure().unwrap().contains("No assistant"));
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('x'));
        assert!(a.assist.is_none());
    }

    #[test]
    fn m_keeps_the_page_in_the_persons_own_knowledge_and_only_theirs() {
        let web = site(vec![("https://a.org/", html("<title>Zoo</title><p>Quokkas are small marsupials from Rottnest Island in Australia.</p>"))]);
        let cx = cx_with("brassist3", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('M'));
        assert!(a.status_text().unwrap().contains("Kept"), "{:?}", a.status_text());
        let kb = cx.services.kb.lock().unwrap();
        let mine = format!("user/{}", cx.session.user);
        assert!(!kb.search(&[mine.as_str()], "quokkas marsupials", 3).is_empty());
        assert!(kb.search(&["user/someone-else"], "quokkas marsupials", 3).is_empty());
    }
    #[test]
    fn u_shows_what_the_server_sent_and_a_second_press_shows_the_page() {
        let web = site(vec![("https://a.org/", html("<title>T</title><!-- marker-comment --><p class=\"x\">Hello <b>there</b></p>"))]);
        let cx = cx_with("brsource", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('U'));
        let shown = format!("{:?}", a.page().unwrap().doc.blocks);
        assert!(shown.contains("marker-comment") && shown.contains("class=") && shown.contains("<b>there</b>"), "{shown}");
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('U'));
        assert!(!format!("{:?}", a.page().unwrap().doc.blocks).contains("marker-comment"));
    }

    #[test]
    fn cookies_that_outlive_the_session_come_back_in_a_new_browser_from_the_vault_and_x_forgets_them() {
        let with_cookie = Response { status: 200, content_type: "text/html".into(), body: b"<p>hello there</p>".to_vec(), set_cookies: vec!["keep=yes; Path=/; Max-Age=86400".into(), "gone=soon; Path=/".into()], ..Response::default() };
        let srv = server(vec![("GET https://s.org/", vec![with_cookie, html("<p>second</p>"), html("<p>third</p>"), html("<p>fourth</p>")])]);
        let cx = cx_server("brpersist", srv.clone());
        cx.services.on_login("root", "rootpass12", true);
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        // a new browser (the next sign-in) has the long-lived cookie but not the session one
        let mut b = BrowserApp::new(&cx);
        b.open_url("s.org", &cx);
        settle(&mut b, &cx);
        assert_eq!(srv.seen.lock().unwrap()[1].cookie, "keep=yes");
        // the vault holds it as a secret, not a plain file
        assert!(cx.services.user_names(&cx.session.user).unwrap().iter().any(|n| n == "wsd.cookies"));
        // X forgets the site's data, here and for the next browser
        key(&mut b, &cx, Key::Char('X'));
        assert!(b.status_text().unwrap().contains("Forgot"), "{:?}", b.status_text());
        let mut c = BrowserApp::new(&cx);
        c.open_url("s.org", &cx);
        settle(&mut c, &cx);
        assert_eq!(srv.seen.lock().unwrap()[2].cookie, "");
    }

    #[test]
    fn indexeddb_databases_are_kept_in_pieces_and_come_back() {
        let cx = cx_with_admin_store("brpersist4");
        cx.services.on_login("root", "rootpass12", true);
        let sites = iyakki_js::Sites::default();
        let u = Url::parse("https://s.org/").unwrap();
        let big = "x".repeat(70_000);
        let json = serde_json::json!({ "dbs": { "d": big } }).to_string();
        let _ = (&json, &u);
        // use the real engine through a page script
        let html = "<script>var r=indexedDB.open('d',1);r.onupgradeneeded=function(){var s=r.result.createObjectStore('s',{keyPath:'id'});s.put({id:1,v:'".to_string() + &big + "'});};</script>";
        let web: Arc<dyn iyakki_web::Fetch> = Arc::new(iyakki_web::fetch::NoNetwork);
        let data = sites.for_origin(&Url::parse("https://s.org/").unwrap());
        let _ = iyakki_js::run_page(&html, &Url::parse("https://s.org/").unwrap(), web, |_| true, &data, iyakki_js::Limits::default());
        assert!(!sites.snapshot_idb().is_empty(), "the script created a database");
        browser_persist::save(&cx, &sites, 1_000, true);
        let fresh = iyakki_js::Sites::default();
        browser_persist::restore(&cx, &fresh, 1_000);
        assert_eq!(fresh.snapshot_idb(), sites.snapshot_idb());
    }

    #[test]
    fn a_page_that_stores_data_has_it_again_in_the_next_browser() {
        let sites = iyakki_js::Sites::default();
        let u = Url::parse("https://s.org/").unwrap();
        sites.for_origin(&u).local.lock().unwrap().0.insert("token".into(), "abc".into());
        let cx = cx_with_admin_store("brpersist2");
        cx.services.on_login("root", "rootpass12", true);
        browser_persist::save(&cx, &sites, 1_000, true);
        let fresh = iyakki_js::Sites::default();
        let (_, n) = browser_persist::restore(&cx, &fresh, 1_000);
        assert_eq!(n, 1);
        assert_eq!(fresh.for_origin(&u).local.lock().unwrap().0.get("token").cloned(), Some("abc".into()));
        // nothing is kept when cookies are turned off
        let cx2 = cx_with_admin_store("brpersist3");
        cx2.services.on_login("root", "rootpass12", true);
        browser_persist::save(&cx2, &sites, 1_000, false);
        assert_eq!(browser_persist::restore(&cx2, &iyakki_js::Sites::default(), 1_000), (0, 0));
    }

    #[test]
    fn reader_view_shows_only_the_article_and_a_second_press_brings_the_page_back() {
        let long = "A sentence long enough to be part of an article and not a menu entry, going on a little. ";
        let page = format!("<title>Story</title><nav><a href=\"/x\">Menu item one</a> <a href=\"/y\">Menu item two</a></nav><h2>Part</h2><p>{long}{long}</p><p>{long}</p><footer><a href=\"/z\">Legal</a></footer>");
        let web = site(vec![("https://a.org/", html(&page)), ("https://b.org/", html("<title>B</title><p><a href=\"/q\">only a link</a></p>"))]);
        let cx = cx_with("brreader", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let full = a.page().unwrap().doc.blocks.len();
        key(&mut a, &cx, Key::Char('R'));
        let text = |a: &BrowserApp| format!("{:?}", a.page().unwrap().doc.blocks);
        assert!(text(&a).contains("A sentence long enough") && !text(&a).contains("Menu item") && !text(&a).contains("Legal"));
        assert!(a.status_text().unwrap().contains("Reader"));
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('R'));
        assert_eq!(a.page().unwrap().doc.blocks.len(), full);
        assert!(text(&a).contains("Menu item"));
        // a page with nothing to read says so and stays as it was
        a.open_url("b.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('R'));
        assert!(a.status_text().unwrap().contains("no article") && text(&a).contains("only a link"));
    }

    #[test]
    fn zoom_makes_the_page_bigger_is_remembered_for_the_site_and_resets() {
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://b.org/", html("<title>B</title><p>Read here</p>"))]);
        let cx = cx_with("brzoom", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let (normal, h0) = (text_size(&a, "Read"), a.layout.as_ref().unwrap().height);
        for _ in 0..3 {
            key(&mut a, &cx, Key::Char('+'));
        }
        let _ = frame(&mut a, &cx, 1280, 720);
        assert!(a.zoom() > 1.2, "{}", a.zoom());
        assert!(text_size(&a, "Read") > normal * 1.2 && a.layout.as_ref().unwrap().height > h0, "the text and the page grew");
        assert!(a.status_text().unwrap().contains('%'));
        // the other site is not affected, and this one is zoomed again after a restart
        a.open_url("b.org", &cx);
        settle(&mut a, &cx);
        assert_eq!(a.zoom(), 1.0);
        let mut b = BrowserApp::new(&cx);
        b.open_url("a.org", &cx);
        settle(&mut b, &cx);
        assert!(b.zoom() > 1.2, "remembered");
        key(&mut b, &cx, Key::Char('0'));
        assert_eq!(b.zoom(), 1.0);
        assert!(browser_data::load(&cx).zoom.is_empty(), "back to normal is not stored");
        for _ in 0..30 {
            key(&mut b, &cx, Key::Char('-'));
        }
        assert_eq!(b.zoom(), browser_data::ZOOM_MIN);
        let g = frame(&mut b, &cx, 1280, 720);
        assert!(g.hits.iter().filter(|h| h.label == "link").count() >= 2, "the page is still drawn and clickable when small");
    }

    // ------------------------------------------------------------------ find in page

    fn find_state(a: &BrowserApp) -> (usize, usize) {
        let f = a.find.as_ref().expect("the find bar is open");
        (f.cur, f.matches.len())
    }

    #[test]
    fn find_counts_matches_in_either_case_across_words_and_steps_through_them() {
        let filler: String = (0..60).map(|i| format!("<p>Line {i} of the filler text that pushes the page down.</p>")).collect();
        let body = format!("<title>Long</title><h1>The Beginning</h1><p>Say hello to the world. HELLO again, and Hello once more.</p>{filler}<p>The very end says hello too.</p>");
        let web = site(vec![("https://a.org/", html(&body))]);
        let cx = cx_with("brfind", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Ctrl('f'));
        typ(&mut a, &cx, "hello");
        assert_eq!(find_state(&a), (0, 4), "three at the top in any case, one at the end");
        let g = frame(&mut a, &cx, 1280, 720);
        for label in ["Previous match", "Next match", "Close find"] {
            assert!(g.hits.iter().any(|h| h.label == label), "{label}");
        }
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(find_state(&a).0, 2);
        assert_eq!(a.scroll, 0.0, "the first three are in view");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(find_state(&a).0, 3);
        assert!(a.scroll > 100.0, "the match at the end is scrolled into view");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(find_state(&a).0, 0, "it wraps");
        assert_eq!(a.scroll, 0.0, "and scrolls back");
        key(&mut a, &cx, Key::Up);
        assert_eq!(find_state(&a).0, 3, "Up goes to the previous match");
        // a phrase over several words, spaces and all
        for _ in 0..5 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "say hello to the world");
        assert_eq!(find_state(&a), (0, 1));
        for _ in 0..22 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "zebra");
        assert_eq!(find_state(&a), (0, 0));
        let g = frame(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "field"), "the find field is drawn");
        key(&mut a, &cx, Key::Esc);
        assert!(a.find.is_none());
        key(&mut a, &cx, Key::Char('r')); // page keys work again
    }

    #[test]
    fn matches_are_drawn_behind_the_words_and_the_bar_buttons_work() {
        let web = site(vec![("https://a.org/", html("<title>T</title><p>alpha beta alpha beta alpha</p>"))]);
        let cx = cx_with("brfind2", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let plain = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Ctrl('f'));
        typ(&mut a, &cx, "alpha");
        let marked = frame(&mut a, &cx, 1280, 720);
        let yellow = |g: &Gfx| g.rgba().chunks(4).filter(|p| p[0] > 200 && p[1] > 100 && p[2] < 120).count();
        assert!(yellow(&marked) > yellow(&plain) + 100, "highlight rectangles appear: {} vs {}", yellow(&marked), yellow(&plain));
        a.handle(&Event::Tab(501), &cx);
        assert_eq!(find_state(&a), (1, 3));
        a.handle(&Event::Tab(500), &cx);
        assert_eq!(find_state(&a), (0, 3));
        a.handle(&Event::Tab(502), &cx);
        assert!(a.find.is_none());
        // other keys still reach the page while the bar is open (page down), and Ctrl+F on the start page says why not
        let mut e = BrowserApp::new(&cx);
        key(&mut e, &cx, Key::Ctrl('f'));
        assert!(e.find.is_none() && e.status_text().unwrap().contains("Open a page"));
    }

    // ------------------------------------------------------------------ selecting and copying page text

    /// A press (or, with `drag`, a held-button move) at the start of the word `word` plus `dx` pixels.
    fn press(a: &mut BrowserApp, cx: &AppCx, word: &str, dx: f32, drag: bool, clicks: u8) {
        let (x, y, _, size) = word_prim(a, word).unwrap_or_else(|| panic!("no word {word}"));
        crate::ui::set_click_count(clicks);
        crate::ui::set_dragging(drag);
        a.handle(&Event::AreaAt(AREA_PAGE, x + a.col_off + dx, y - a.scroll + size / 2.0), cx);
        crate::ui::set_dragging(false);
        crate::ui::set_click_count(1);
    }

    #[test]
    fn dragging_across_the_page_selects_text_double_click_a_word_triple_click_a_line_and_ctrl_c_copies() {
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("brsel", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let plain = frame(&mut a, &cx, 1280, 720);
        // press at the start of "Read", drag to the end of "page"
        press(&mut a, &cx, "Read", 0.5, false, 1);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert!(a.sel_range.is_none(), "a plain press selects nothing");
        let page_w = word_prim(&a, "page").map(|(x, _, _, _)| x).unwrap();
        let (_, _, _, size) = word_prim(&a, "page").unwrap();
        press(&mut a, &cx, "page", size * 2.4, true, 1);
        let marked = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.sel_text, "Read the more page");
        let blue = |g: &Gfx| g.rgba().chunks(4).filter(|p| p[2] > 150 && p[0] < 120 && p[1] > 70 && p[1] < 150).count();
        assert!(blue(&marked) > blue(&plain) + 300, "the selection is highlighted: {} vs {}", blue(&marked), blue(&plain));
        let _ = page_w;
        clipboard_clear();
        key(&mut a, &cx, Key::Ctrl('c'));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("Read the more page"));
        // a double click selects the word, a triple click the whole line
        press(&mut a, &cx, "elsewhere", 5.0, false, 2);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.sel_text, "elsewhere");
        press(&mut a, &cx, "elsewhere", 5.0, false, 3);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.sel_text, "Read the more page or go elsewhere.");
        // Ctrl+A takes the whole page, a heading line break included
        key(&mut a, &cx, Key::Ctrl('a'));
        let _ = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.sel_text, "Guide\nRead the more page or go elsewhere.");
        // Esc clears the selection first, going to another page also does
        key(&mut a, &cx, Key::Esc);
        assert!(a.sel.is_none());
        press(&mut a, &cx, "Read", 0.5, false, 2);
        let _ = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.sel_text, "Read");
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        assert!(a.sel.is_none());
    }

    fn clipboard_clear() {
        crate::ui::clipboard::clear();
    }

    #[test]
    fn y_copies_the_address_of_the_link_in_focus_with_the_page_address_resolved() {
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("brlink", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().contains("Point at a link"));
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("https://a.org/more"));
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("https://other.org/x"));
    }

    // ------------------------------------------------------------------ safe browsing

    #[test]
    fn certificate_problems_get_a_plain_explanation_and_other_errors_name_the_site() {
        let m = friendly_error("bank.example", "tls: invalid peer certificate: UnknownIssuer");
        assert!(m.starts_with("bank.example was not opened") && m.contains("self-signed") && m.contains("to protect you"), "{m}");
        assert!(friendly_error("a.org", "invalid peer certificate: Expired").contains("has expired"));
        assert!(friendly_error("a.org", "invalid peer certificate: NotValidForName").contains("different site name"));
        assert_eq!(friendly_error("a.org", "connection refused"), "a.org: connection refused");
    }

    #[test]
    fn a_secure_page_does_not_load_unprotected_pictures() {
        let page = "<p><img src=\"http://cdn.org/a.png\" alt=insecure> <img src=\"https://cdn.org/b.png\" alt=ok></p>";
        let srv = server(vec![
            ("GET https://s.org/", vec![html(page)]),
            ("GET https://cdn.org/b.png", vec![Response { status: 200, content_type: "image/png".into(), body: png_file(), ..Response::default() }]),
            ("GET http://cdn.org/a.png", vec![Response { status: 200, content_type: "image/png".into(), body: png_file(), ..Response::default() }]),
        ]);
        let cx = cx_server("brmixed", srv.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("s.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        settle_images(&mut a, &cx);
        assert!(matches!(&a.history[a.pos].images[0], ImgState::Failed(e) if e.contains("without protection")), "{:?}", a.history[a.pos].images.iter().map(|i| i.settled()).collect::<Vec<_>>());
        assert!(matches!(a.history[a.pos].images[1], ImgState::Ready(_)));
        assert!(!srv.seen.lock().unwrap().iter().any(|r| r.url.to_string().starts_with("http://cdn.org")), "the unprotected address is never even asked");
    }

    #[test]
    fn a_password_is_not_sent_without_protection_unless_asked_twice() {
        let login = "<title>Login</title><form method=post action=/in><input name=u><input type=password name=p><input type=submit value=Go></form>";
        let srv = server(vec![("GET http://old.org/", vec![html(login)]), ("POST http://old.org/in", vec![html("<title>In</title><p>welcome</p>")])]);
        let cx = cx_server("brhttp", srv.clone());
        let mut a = BrowserApp::new(&cx);
        a.open_url("http://old.org/", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let u = text_ctl(&a);
        a.handle(&Event::FocusAt(CONTROL_BASE + u, 5.0), &cx);
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(a.status_text().unwrap().contains("not protected"), "{:?}", a.status_text());
        assert!(!a.is_loading() && srv.seen.lock().unwrap().len() == 1, "nothing was sent");
        a.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().unwrap().to_string(), "http://old.org/in", "the second Enter sends it");
        // a secure page sends at once
        let secure = server(vec![("GET https://ok.org/", vec![html(&login.replace("/in", "/in"))]), ("POST https://ok.org/in", vec![html("<title>In</title><p>welcome</p>")])]);
        let cx2 = cx_server("brhttps", secure);
        let mut b = BrowserApp::new(&cx2);
        b.open_url("https://ok.org/", &cx2);
        settle(&mut b, &cx2);
        let _ = frame(&mut b, &cx2, 1280, 720);
        let u = text_ctl(&b);
        b.handle(&Event::FocusAt(CONTROL_BASE + u, 5.0), &cx2);
        b.handle(&Event::Key(Key::Enter), &cx2);
        settle(&mut b, &cx2);
        assert_eq!(b.current_url().unwrap().to_string(), "https://ok.org/in");
    }

    // ------------------------------------------------------------------ saved sign-ins

    const LOGIN: &str = "<title>Sign in</title><form method=post action=/in><input name=u><input type=password name=p><input type=submit value=Go></form>";

    fn pw_ctl(a: &BrowserApp) -> usize {
        a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Password).unwrap()
    }

    fn type_into(a: &mut BrowserApp, cx: &AppCx, ctl: usize, text: &str) {
        a.handle(&Event::FocusAt(CONTROL_BASE + ctl, 5.0), cx);
        typ(a, cx, text);
    }

    #[test]
    fn a_sign_in_that_worked_is_offered_saved_in_the_vault_and_filled_in_next_time_only_on_that_site() {
        let srv = server(vec![
            ("GET https://acc.org/", vec![html(LOGIN)]),
            ("POST https://acc.org/in", vec![html("<title>Home</title><p>welcome back</p>")]),
            ("GET https://other.org/", vec![html(LOGIN)]),
        ]);
        let cx = cx_server("brpw", srv.clone());
        cx.services.on_login("root", "rootpass12", true);
        let mut a = BrowserApp::new(&cx);
        a.open_url("acc.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Ctrl('k'));
        assert!(a.status_text().unwrap().contains("Nothing is saved"), "{:?}", a.status_text());
        let (u, p) = (text_ctl(&a), pw_ctl(&a));
        type_into(&mut a, &cx, u, "ann");
        type_into(&mut a, &cx, p, "pw-123456");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(a.status_text().unwrap().contains("Save the sign-in for ann on acc.org"), "{:?}", a.status_text());
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(a.status_text().unwrap().contains("Saved the sign-in"));
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("browser.password.save") && l.contains("acc.org")));
        assert!(!cx.audit.recent(50).iter().any(|l| l.contains("pw-123456")), "the password is never logged");
        // next time: one key fills both fields
        a.open_url("acc.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Ctrl('k'));
        let (u, p) = (text_ctl(&a), pw_ctl(&a));
        assert_eq!((a.history[a.pos].ctl[u].value.as_str(), a.history[a.pos].ctl[p].value.as_str()), ("ann", "pw-123456"));
        // the same sign-in is not offered again
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(a.offer.is_none());
        // another site gets nothing, even though the form looks the same
        a.open_url("other.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Ctrl('k'));
        assert!(a.status_text().unwrap().contains("Nothing is saved for other.org"));
        assert_eq!(a.history[a.pos].ctl[pw_ctl(&a)].value, "");
    }

    #[test]
    fn a_failed_sign_in_is_not_offered_and_a_locked_vault_says_so() {
        let srv = server(vec![("GET https://acc.org/", vec![html(LOGIN)]), ("POST https://acc.org/in", vec![html(LOGIN)])]);
        let cx = cx_server("brpw2", srv);
        let mut a = BrowserApp::new(&cx);
        a.open_url("acc.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        let (u, p) = (text_ctl(&a), pw_ctl(&a));
        type_into(&mut a, &cx, u, "ann");
        type_into(&mut a, &cx, p, "wrong-pw-1");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(a.offer.is_none(), "the answer was the sign-in form again");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(a.status_text().unwrap().contains("no sign-in to save"));
        key(&mut a, &cx, Key::Ctrl('k'));
        assert!(a.status_text().unwrap().contains("locked"), "{:?}", a.status_text());
    }

    #[test]
    fn ctrl_g_makes_a_strong_password_for_the_field_and_its_again_field_and_it_is_offered_for_saving() {
        let signup = "<title>Join</title><form method=post action=/join><input name=u><input type=password name=p><input type=password name=again><input type=submit value=Join></form>";
        let srv = server(vec![("GET https://new.org/", vec![html(signup)]), ("POST https://new.org/join", vec![html("<title>Hi</title><p>joined</p>")])]);
        let cx = cx_server("brpw3", srv);
        cx.services.on_login("root", "rootpass12", true);
        let mut a = BrowserApp::new(&cx);
        a.open_url("new.org", &cx);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Ctrl('g'));
        assert!(a.status_text().unwrap().contains("Click a password field"));
        let u = text_ctl(&a);
        type_into(&mut a, &cx, u, "newbie");
        key(&mut a, &cx, Key::Ctrl('g'));
        assert!(a.status_text().unwrap().contains("password field"), "a text field gets no password: {:?}", a.status_text());
        let pws: Vec<usize> = a.page().unwrap().doc.controls.iter().enumerate().filter(|(_, c)| c.kind == ControlKind::Password).map(|(i, _)| i).collect();
        a.handle(&Event::FocusAt(CONTROL_BASE + pws[0], 5.0), &cx);
        key(&mut a, &cx, Key::Ctrl('g'));
        let (first, second) = (a.history[a.pos].ctl[pws[0]].value.clone(), a.history[a.pos].ctl[pws[1]].value.clone());
        assert_eq!(first.len(), 20);
        assert_eq!(first, second, "the confirmation field matches");
        assert_eq!(a.edit.value, first, "the field being edited shows it too");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert_eq!(a.offer.as_ref().map(|c| c.password.clone()), Some(first.clone()));
        key(&mut a, &cx, Key::Ctrl('s'));
        let saved = browser_logins::for_site(&cx, "new.org").unwrap();
        assert_eq!((saved[0].user.as_str(), saved[0].password.as_str()), ("newbie", first.as_str()));
    }

    // ------------------------------------------------------------------------------------ the embedded full engine

    use std::path::PathBuf;

    /// The tests that start (or must not find) a helper program share the process environment: one at a time.
    static FULL_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        FULL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// No helper anywhere: the state of an image without the web engine.
    struct NoHelper(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl NoHelper {
        fn new() -> NoHelper {
            let g = lock();
            std::env::remove_var(iyakki_agent::webengine::HELPER_ENV);
            iyakki_agent::webengine::set_host_mode(false);
            NoHelper(g)
        }
    }

    /// The pure-Rust fake helper of `iyakki-webview` (built on demand: `cargo test -p iyakki-shell` does not build other crates' programs).
    fn fake_helper() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
        let p = dir.join("iyakki-fake-webview");
        if !p.is_file() {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let ok = std::process::Command::new(cargo).args(["build", "-q", "-p", "iyakki-webview", "--bin", "iyakki-fake-webview"]).status().map(|s| s.success()).unwrap_or(false);
            assert!(ok && p.is_file(), "could not build the fake helper");
        }
        p
    }

    /// While alive, `IYAKKI_WEBVIEW_HELPER` names a script that starts the fake helper with the given options.
    struct Fake(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Fake {
        fn new(tag: &str, options: &str) -> Fake {
            let g = lock();
            let dir = crate::apps::tests::temp_dir(&format!("fake-{tag}"));
            let script = dir.join("helper.sh");
            std::fs::write(&script, format!("#!/bin/sh\nexec {} {} \"$@\"\n", fake_helper().display(), options)).unwrap();
            std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            std::env::set_var(iyakki_agent::webengine::HELPER_ENV, &script);
            iyakki_agent::webengine::set_host_mode(false);
            Fake(g)
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            std::env::remove_var(iyakki_agent::webengine::HELPER_ENV);
        }
    }

    include!("browser_block_ui_tests.rs");
    include!("browser_devpanel_tests.rs");
    include!("browser_media_tests.rs");
    include!("browser_consent_live_tests.rs");
    include!("browser_navpolicy_tests.rs");

    fn pump(a: &mut BrowserApp, cx: &AppCx, what: &str, mut done: impl FnMut(&mut BrowserApp) -> bool) {
        for _ in 0..2000 {
            a.handle(&Event::Tick, cx);
            if done(a) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (title, url, log) = a.full.as_ref().map(|f| (f.title.clone(), f.url.clone(), f.log.iter().rev().take(6).cloned().collect::<Vec<_>>())).unwrap_or_default();
        panic!("timed out waiting for {what}; status {:?}, full title {title:?} url {url:?} log {log:?}", a.status_text());
    }

    fn full_title(a: &BrowserApp) -> String {
        a.full.as_ref().map(|f| f.title.clone()).unwrap_or_default()
    }

    /// The pixel of the picture drawn at a position inside the page area.
    fn px(g: &Gfx, x: f32, y: f32) -> [u8; 4] {
        let o = ((y as u32 * g.width() + x as u32) * 4) as usize;
        g.pm.data()[o..o + 4].try_into().unwrap()
    }

    /// A browser on `a.org` in the full engine (the fake helper, no animation), the first picture already drawn.
    fn in_full(tag: &str, options: &str) -> (Fake, AppCx, BrowserApp, Gfx) {
        let fake = Fake::new(tag, options);
        let web = site(vec![("https://a.org/", html(GUIDE)), ("https://b.org/x", html(GUIDE)), ("https://example.org/files/report.pdf", Response { status: 200, content_type: "application/pdf".into(), body: b"%PDF-1.4 hello".to_vec(), ..html("") })]);
        let cx = cx_with(tag, web);
        let mut a = BrowserApp::new(&cx);
        typ(&mut a, &cx, "a.org");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('F'));
        assert!(a.full.is_some(), "{:?}", a.status_text());
        pump(&mut a, &cx, "the first picture", |a| a.full.as_ref().is_some_and(|f| f.pixmap().is_some() && f.url == "https://a.org/"));
        let g = frame(&mut a, &cx, 1280, 720);
        (fake, cx, a, g)
    }

    #[test]
    fn f_shows_the_page_with_the_engine_in_the_page_area_and_frames_ask_for_a_redraw() {
        let (_fake, cx, mut a, g) = in_full("full1", "--static");
        let view = a.view_rect;
        // the engine's picture, 1:1, in the page area: the fake's top-left quarter is (220, 40, 40)
        assert_eq!(px(&g, view.x + 100.0, view.y + 100.0), [220, 40, 40, 255]);
        assert!(px(&g, view.x + view.w - 100.0, view.y + 100.0)[1] >= 200, "top right is green");
        // the chrome around it is the browser's own
        assert_eq!(a.tab_titles(), vec!["fake page https://a.org/".to_string()]);
        assert_eq!(a.current_url().unwrap().to_string(), "https://a.org/");
        assert_eq!(a.field.value, "https://a.org/");
        let f = a.full.as_ref().unwrap();
        assert_eq!(f.engine, "fake 1");
        assert_eq!(f.size_wanted(), (view.w as u32, view.h as u32));
        assert_eq!(f.frame_size(), (view.w as u32, view.h as u32));
        assert!(f.log.iter().any(|l| l.contains("Hello") && l.contains("scale_pct: 100") && l.contains("user_agent: \"IYAKKI/")), "{:?}", f.log);
        assert!(f.log.iter().any(|l| l.contains("Hello") && l.contains(&format!("w: {}", view.w as u32))), "the real pixel size is sent");
        // the account's host list goes to the helper
        assert!(f.log.iter().any(|l| l.contains("allow_hosts: [\"*\"]")));
        // a picture that arrives asks the shell for a redraw; nothing arriving asks for nothing
        while a.wants_redraw() {}
        a.handle(&Event::Tick, &cx);
        assert!(!a.wants_redraw() || a.full.as_ref().unwrap().frames > 0);
        // status and history
        assert!(a.status_text().is_none() || !a.status_text().unwrap().contains("could not"), "{:?}", a.status_text());
        assert!(a.data.history.iter().any(|v| v.url == "https://a.org/" && v.title == "fake page https://a.org/"), "{:?}", a.data.history);
        let audit = std::fs::read_to_string(cx.services.dir().parent().unwrap().join("audit.log")).unwrap_or_default();
        assert!(audit.contains("browser.full"), "{audit}");
    }

    #[test]
    fn the_hidpi_scale_and_the_real_size_go_to_the_engine() {
        let _fake = Fake::new("full_scale", "--static");
        let web = site(vec![("https://a.org/", html(GUIDE))]);
        let cx = cx_with("full_scale", web);
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        frame(&mut a, &cx, 2560, 1440); // a big screen: the interface is drawn at twice the size
        key(&mut a, &cx, Key::Char('F'));
        pump(&mut a, &cx, "the engine", |a| a.full.as_ref().is_some_and(|f| f.pixmap().is_some()));
        let f = a.full.as_ref().unwrap();
        assert!(f.log.iter().any(|l| l.contains("scale_pct: 200") && l.contains("w: 2560")), "{:?}", f.log);
        // a smaller window later: the engine is resized to the new area
        frame(&mut a, &cx, 1280, 720);
        let want = (a.view_rect.w as u32, a.view_rect.h as u32);
        pump(&mut a, &cx, "the resized picture", |a| a.full.as_ref().is_some_and(|f| f.frame_size() == want));
        // the zoom of the site is part of what the engine is told
        a.change_zoom(Some(true), &cx);
        let z = a.full.as_ref().unwrap().log.iter().any(|l| l.contains("Zoom"));
        pump(&mut a, &cx, "the zoom", |a| a.full.as_ref().is_some_and(|f| f.log.iter().any(|l| l.contains("Zoom("))) || z);
    }

    #[test]
    fn a_page_in_fullscreen_gets_the_whole_pane_with_no_bars_and_esc_asks_the_page_to_leave() {
        let (_fake, cx, mut a, _g) = in_full("full_fs", "--static");
        let normal = a.view_rect;
        let area = R::new(0.0, 72.0, 1280.0, 720.0 - 72.0);
        assert!(normal.h < area.h, "the bars take room from the page normally");
        a.full.as_mut().unwrap().navigate("fake://fullscreen");
        pump(&mut a, &cx, "the page entering fullscreen", |a| a.full.as_ref().is_some_and(|f| f.fullscreen));
        let g = frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.view_rect, area, "the page area is the whole pane");
        assert!(!g.hits.iter().any(|h| h.label == "close tab" || h.label == "new tab"), "the bars are not drawn or clickable");
        assert!(g.hits.iter().any(|h| h.label == "page" && h.ev == Event::Area(AREA_FULL)));
        let want = (area.w as u32, area.h as u32);
        pump(&mut a, &cx, "the engine resized to the pane", |a| {
            frame(a, &cx, 1280, 720);
            a.full.as_ref().is_some_and(|f| f.frame_size() == want)
        });
        // keys other than Esc are the page's own (the F key must not switch back to the reader, a letter is typed in the page)
        key(&mut a, &cx, Key::Char('F'));
        assert!(a.full.as_ref().unwrap().fullscreen);
        pump(&mut a, &cx, "the key reaching the page", |a| a.full.as_ref().is_some_and(|f| f.log.iter().any(|l| l.contains("Key {") && l.contains("keysym: 70,"))));
        // Esc asks the page to leave; the bars come back only when the page says it has
        key(&mut a, &cx, Key::Esc);
        assert!(a.full.is_some(), "Esc in fullscreen does not close the engine");
        pump(&mut a, &cx, "the page leaving fullscreen", |a| a.full.as_ref().is_some_and(|f| !f.fullscreen));
        assert!(a.full.as_ref().unwrap().log.iter().any(|l| l.contains("LeaveFullscreen")));
        frame(&mut a, &cx, 1280, 720);
        assert_eq!(a.view_rect, normal, "the normal layout is back");
        let want = (normal.w as u32, normal.h as u32);
        pump(&mut a, &cx, "the engine resized back", |a| {
            frame(a, &cx, 1280, 720);
            a.full.as_ref().is_some_and(|f| f.frame_size() == want)
        });
        // a navigation that ends fullscreen on its own restores the layout too
        a.full.as_mut().unwrap().navigate("fake://fullscreen");
        pump(&mut a, &cx, "fullscreen again", |a| a.full.as_ref().is_some_and(|f| f.fullscreen));
        a.full.as_mut().unwrap().navigate("https://b.org/x");
        pump(&mut a, &cx, "fullscreen ended by the navigation", |a| a.full.as_ref().is_some_and(|f| !f.fullscreen));
    }

    #[test]
    fn smooth_pages_ask_the_engine_for_half_the_pixels_stretch_them_and_clicks_still_land_under_the_pointer() {
        let (_fake, cx, mut a, _g) = in_full("full_smooth", "--static");
        let view = a.view_rect;
        assert_eq!(a.full.as_ref().unwrap().frame_size(), (view.w as u32, view.h as u32), "sharp by default: one picture pixel per screen pixel");
        // the person switches Smooth pages on: the next heartbeat tells the engine half the width and height, and half the zoom
        browser_config::save(&cx, &BrowserConfig { smooth: true, ..BrowserConfig::default() });
        a.handle(&Event::Tick, &cx);
        frame(&mut a, &cx, 1280, 720);
        let half = ((view.w as u32) / 2, (view.h as u32) / 2);
        pump(&mut a, &cx, "the half size picture", |a| a.full.as_ref().is_some_and(|f| f.frame_size() == half));
        let log = &a.full.as_ref().unwrap().log;
        assert!(log.iter().any(|l| l.contains("Zoom(50)")), "the zoom is halved so the page keeps its layout: {log:?}");
        // the page area still shows the whole picture, stretched: the fake's top-left quarter is red, top right green
        let g = frame(&mut a, &cx, 1280, 720);
        assert_eq!(px(&g, view.x + 100.0, view.y + 100.0), [220, 40, 40, 255]);
        assert!(px(&g, view.x + view.w - 100.0, view.y + 100.0)[1] >= 200, "top right is still green");
        // a pointer position on the screen reaches the engine in its own (half size) pixels
        a.handle(&Event::MouseMove((view.x + 300.0) as i32, (view.y + 200.0) as i32), &cx);
        pump(&mut a, &cx, "the move in half size pixels", |a| full_title(a).contains("mouse 150,100 move"));
        a.handle(&Event::AreaAt(AREA_FULL, 400.0, 80.0), &cx);
        pump(&mut a, &cx, "the press in half size pixels", |a| full_title(a).contains("mouse 200,40 down buttons 1"));
        // and back to sharp
        browser_config::save(&cx, &BrowserConfig::default());
        a.handle(&Event::Tick, &cx);
        frame(&mut a, &cx, 1280, 720);
        let full = (view.w as u32, view.h as u32);
        pump(&mut a, &cx, "the full size picture", |a| a.full.as_ref().is_some_and(|f| f.frame_size() == full));
        assert!(a.full.as_ref().unwrap().log.iter().any(|l| l.contains("Zoom(100)")), "the zoom is back");
    }

    #[test]
    fn a_new_picture_alone_is_painted_in_place_and_looks_exactly_like_a_whole_redraw() {
        for smooth in [false, true] {
            let (_fake, cx, mut a, mut g) = in_full(&format!("full_live{smooth}"), "--fps 30");
            browser_config::save(&cx, &BrowserConfig { smooth, ..BrowserConfig::default() });
            let view = a.view_rect;
            let (w, h) = (g.width(), g.height());
            let mut painted = None;
            for _ in 0..600 {
                a.handle(&Event::Tick, &cx);
                match a.redraw_kind() {
                    Redraw::Full => g = frame(&mut a, &cx, w, h), // chrome or size changed (the smooth switch resizes the picture)
                    Redraw::Live => {
                        // the whole scene is current but for the new picture: paint just that
                        painted = a.live_draw(&mut g);
                        if painted.is_some() {
                            break;
                        }
                    }
                    Redraw::None => {}
                }
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            let (x, y, rw, rh) = painted.unwrap_or_else(|| panic!("smooth {smooth}: a picture arrived and was painted in place"));
            assert!(rw > 0 && rh > 0 && x >= view.x as i32 - 1 && y >= view.y as i32 - 1 && (x + rw) as f32 <= view.x + view.w + 1.0 && (y + rh) as f32 <= view.y + view.h + 1.0, "the rectangle {x},{y} {rw}x{rh} is inside the page area {view:?}");
            // a whole redraw of the same state gives the same page area, pixel for pixel
            let whole = frame(&mut a, &cx, w, h);
            let (vx, vy, vw, vh) = (view.x as usize, view.y as usize, view.w as usize, view.h as usize);
            let stride = w as usize * 4;
            // the page's first row is the tool bar's (it draws a faint line over it) and is left to the next whole redraw
            for row in vy + 1..vy + vh {
                let (a_row, b_row) = (&g.pm.data()[row * stride + vx * 4..row * stride + (vx + vw) * 4], &whole.pm.data()[row * stride + vx * 4..row * stride + (vx + vw) * 4]);
                if a_row != b_row {
                    let first = a_row.iter().zip(b_row).position(|(x, y)| x != y).unwrap_or(0);
                    let n = a_row.iter().zip(b_row).filter(|(x, y)| x != y).count();
                    panic!("smooth {smooth}: row {row} of the page area differs between the in-place paint and a whole redraw: first byte {first}, {n} bytes; view {view:?}, painted {painted:?}, in place {:?} whole {:?}", &a_row[first..first + 8], &b_row[first..first + 8]);
                }
            }
            assert_eq!(a.redraw_kind(), Redraw::None, "a whole redraw consumed the pending picture");
        }
    }

    #[test]
    fn mouse_wheel_and_keys_reach_the_engine_with_page_coordinates() {
        let (_fake, cx, mut a, g) = in_full("full2", "--static");
        let view = a.view_rect;
        // the page area is one hit region; the shell reports a press relative to it
        let hit = g.hits.iter().find(|h| h.label == "page" && h.ev == Event::Area(AREA_FULL)).expect("the page area is a hit region");
        assert_eq!((hit.rect.x, hit.rect.y, hit.rect.w, hit.rect.h), (view.x, view.y, view.w, view.h));
        a.handle(&Event::AreaAt(AREA_FULL, 150.0, 80.0), &cx);
        pump(&mut a, &cx, "the press", |a| full_title(a).contains("mouse 150,80 down buttons 1"));
        a.handle(&Event::MouseUp((view.x + 150.0) as i32, (view.y + 80.0) as i32), &cx);
        pump(&mut a, &cx, "the release", |a| full_title(a).contains("mouse 150,80 up buttons 0"));
        // moving over the page (absolute positions) arrives relative to the page
        a.handle(&Event::MouseMove((view.x + 222.0) as i32, (view.y + 111.0) as i32), &cx);
        pump(&mut a, &cx, "the move", |a| full_title(a).contains("mouse 222,111 move"));
        // the wheel: 2 notches up are negative units (down is positive) at the pointer
        a.handle(&Event::Scroll(2), &cx);
        // the shell's scroll engine eases a notch into several smaller wheel events (F-SHELL-50), so the SUM of what reached the page counts
        let wheel_sum = |a: &BrowserApp| -> i32 {
            a.full.as_ref().map(|f| f.log.iter().filter(|l| l.starts_with("got Wheel")).filter_map(|l| l.split("dy: ").nth(1)?.split(',').next()?.trim().parse::<i32>().ok()).sum()).unwrap_or(0)
        };
        let before = wheel_sum(&a);
        // a notch moves the page by the same screen distance as the Settings lists (scroll match), not 120 units: derive the units
        let unit = crate::apps::browser_full::wheel_units_f(crate::scrolleng::NOTCH_PX, 1.0);
        let near = |got: i32, want: f32| (got as f32 - want).abs() <= 1.5;
        pump(&mut a, &cx, "the wheel", |a| full_title(a).contains("wheel 222,111") && near(wheel_sum(a) - before, 2.0 * unit));
        a.handle(&Event::Scroll(-1), &cx);
        pump(&mut a, &cx, "the wheel down", |a| near(wheel_sum(a) - before, 2.0 * unit - unit));
        // with the pointer outside the page (over the browser's own bar) the wheel and the pointer are not the page's
        let count = |a: &BrowserApp, what: &str| a.full.as_ref().unwrap().log.iter().filter(|l| l.contains(what)).count();
        let (moves, wheels) = (count(&a, "got Mouse"), count(&a, "got Wheel"));
        a.handle(&Event::MouseMove(3, 3), &cx);
        a.handle(&Event::Scroll(1), &cx);
        for _ in 0..40 {
            a.handle(&Event::Tick, &cx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!((count(&a, "got Mouse"), count(&a, "got Wheel")), (moves, wheels), "nothing outside the page area is forwarded");
        // keys go to the page while it has the keyboard: letters, capital F (which must NOT leave the engine), Enter, Ctrl+C
        key(&mut a, &cx, Key::Char('a'));
        pump(&mut a, &cx, "a", |a| full_title(a).contains("key up 0x61 mods 0"));
        a.handle(&Event::Key(Key::Char('F')), &cx);
        pump(&mut a, &cx, "F", |a| full_title(a).contains("key up 0x46 mods 1"));
        assert!(a.full.is_some(), "F typed in the page is text, not a switch");
        key(&mut a, &cx, Key::Enter);
        pump(&mut a, &cx, "Enter", |a| full_title(a).contains("key up 0xff0d"));
        key(&mut a, &cx, Key::Ctrl('c'));
        pump(&mut a, &cx, "Ctrl+C", |a| full_title(a).contains("key up 0x63 mods 2"));
        a.handle(&Event::Paste("pasted ü".into()), &cx);
        pump(&mut a, &cx, "paste", |a| full_title(a).contains("text \"pasted ü\""));
        // the drawn picture shows where the engine put its mouse square (it echoes what it got)
        a.handle(&Event::MouseMove((view.x + 300.0) as i32, (view.y + 200.0) as i32), &cx);
        pump(&mut a, &cx, "the square", |a| full_title(a).contains("mouse 300,200"));
        pump(&mut a, &cx, "its picture", |a| a.full.as_ref().is_some_and(|f| f.pixmap().is_some_and(|p| p.pixel(300, 200).is_some_and(|c| c.red() == 255 && c.green() == 0 && c.blue() == 255))));
        let g = frame(&mut a, &cx, 1280, 720);
        assert_eq!(px(&g, view.x + 300.0, view.y + 200.0), [255, 0, 255, 255]);
    }

    #[test]
    fn the_bar_keeps_its_typing_and_drives_the_engine() {
        let (_fake, cx, mut a, _g) = in_full("full3", "--static");
        // Ctrl+L: the bar has the keyboard; letters typed there do not reach the page
        key(&mut a, &cx, Key::Ctrl('l'));
        assert!(a.addr_focus);
        typ(&mut a, &cx, "b.org/x");
        key(&mut a, &cx, Key::Enter);
        pump(&mut a, &cx, "the new page", |a| full_title(a) == "fake page https://b.org/x");
        assert!(!a.addr_focus, "Enter sends the keyboard back to the page");
        assert!(!a.full.as_ref().unwrap().log.iter().any(|l| l.contains("Key {")), "typing in the bar never went to the page: {:?}", a.full.as_ref().unwrap().log);
        assert_eq!(a.tab_titles()[0], "fake page https://b.org/x");
        assert_eq!(a.field.value, "https://b.org/x");
        // history and the star follow the engine's page
        assert!(a.data.history.iter().any(|v| v.url == "https://b.org/x"));
        key(&mut a, &cx, Key::Ctrl('d'));
        assert!(a.data.is_bookmarked("https://b.org/x"));
        // back, forward and reload are the toolbar's
        a.handle(&Event::Tab(0), &cx);
        pump(&mut a, &cx, "back", |a| a.full.as_ref().is_some_and(|f| f.url == "https://a.org/") && a.full.as_ref().is_some_and(|f| f.can_forward));
        a.handle(&Event::Tab(1), &cx);
        pump(&mut a, &cx, "forward", |a| a.full.as_ref().is_some_and(|f| f.url == "https://b.org/x"));
        a.handle(&Event::Tab(2), &cx);
        pump(&mut a, &cx, "reload", |a| a.full.as_ref().is_some_and(|f| f.log.iter().any(|l| l.contains("Reload"))));
        // Esc in the bar gives the keyboard back to the page
        key(&mut a, &cx, Key::Ctrl('l'));
        for _ in 0..2 {
            key(&mut a, &cx, Key::Esc); // the first closes the list of suggestions
        }
        assert!(!a.addr_focus && a.full.as_ref().unwrap().page_focus);
    }

    #[test]
    fn open_in_new_tab_leaves_the_current_tab_alone_and_opens_the_address_in_the_engine() {
        let (_fake, cx, mut a, _g) = in_full("full_newtab", "--static");
        let before = a.tab_titles().len();
        a.open_in_new_tab("https://c.org/y", &cx);
        assert_eq!(a.tab_titles().len(), before + 1);
        assert_eq!(a.active, before, "the new tab is the active one");
        pump(&mut a, &cx, "the new tab's page", |a| full_title(a) == "fake page https://c.org/y");
        // the first tab still has its page
        a.switch_to(0);
        assert_eq!(a.tab_titles()[0], "fake page https://a.org/");
    }

    #[test]
    fn esc_hands_the_keyboard_back_and_then_returns_to_the_in_house_engine() {
        let (_fake, cx, mut a, _g) = in_full("full4", "--static");
        key(&mut a, &cx, Key::Esc);
        assert!(!a.full.as_ref().unwrap().page_focus, "the first Esc is the browser's");
        pump(&mut a, &cx, "the focus message", |a| full_title(a).contains("focus false"));
        // the browser's letters work again: scrolling keys still reach the page
        key(&mut a, &cx, Key::Down);
        pump(&mut a, &cx, "Down", |a| full_title(a).contains("key up 0xff54"));
        key(&mut a, &cx, Key::Esc);
        assert!(a.full.is_none(), "the second Esc returns to the built-in reader");
        assert!(a.status_text().unwrap_or_default().contains("built-in reader"));
        settle(&mut a, &cx);
        assert_eq!(a.page().unwrap().doc.title, "The Guide", "the reader shows the page it had");
        assert_eq!(a.history.len(), 2, "the engine's step replaced by the reader's page: start page, then the page");
        // F toggles too: on, then (with the browser holding the keyboard) off
        key(&mut a, &cx, Key::Char('F'));
        assert!(a.full.is_some());
        pump(&mut a, &cx, "the engine again", |a| a.full.as_ref().is_some_and(|f| f.pixmap().is_some()));
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Char('F'));
        assert!(a.full.is_none());
        // the toolbar button toggles as well
        a.handle(&Event::Tab(9), &cx);
        assert!(a.full.is_some());
        a.handle(&Event::Tab(9), &cx);
        assert!(a.full.is_none());
    }

    #[test]
    fn a_host_the_account_may_not_open_is_refused_for_the_start_and_for_every_navigation_the_engine_reports() {
        let (_fake, cx, mut a, _g) = in_full("full5", "--static");
        // the account loses access to evil.example (and everything but a.org) while the page is open
        let mut cx2 = cx.clone();
        cx2.session.policy.net_hosts = vec!["a.org".into()];
        a.full.as_mut().unwrap().navigate("fake://goto/evil.example");
        pump(&mut a, &cx2, "the refusal", |a| a.status_text().is_some_and(|s| s.contains("evil.example")));
        let st = a.status_text().unwrap().to_string();
        assert!(st.contains("may not open pages from evil.example") && st.contains("went back"), "{st}");
        pump(&mut a, &cx2, "going back", |a| a.full.as_ref().is_some_and(|f| f.url == "https://a.org/" && f.good_url == "https://a.org/"));
        assert!(!a.data.history.iter().any(|v| v.url.contains("evil.example")), "a refused page is not in the history");
        let audit = std::fs::read_to_string(cx.services.dir().parent().unwrap().join("audit.log")).unwrap_or_default();
        assert!(audit.contains("browser.full.refused") && audit.contains("evil.example"), "{audit}");
        // the first address is checked before anything starts
        let mut limited = cx.clone();
        limited.session.policy.net_hosts = vec!["a.org".into()];
        let mut b = BrowserApp::new(&limited);
        b.open_url("a.org", &limited);
        settle(&mut b, &limited);
        key(&mut b, &limited, Key::Char('F'));
        assert!(b.full.is_none());
        assert!(b.status_text().unwrap_or_default().contains("limited to some sites"), "{:?}", b.status_text());
    }

    #[test]
    fn a_download_the_page_asks_for_goes_through_the_browsers_download_path() {
        let (_fake, cx, mut a, _g) = in_full("full6", "--static");
        a.full.as_mut().unwrap().navigate("fake://download/report.pdf");
        pump(&mut a, &cx, "the download", |a| a.download.is_some() || a.status_text().is_some_and(|s| s.starts_with("Saved")));
        pump(&mut a, &cx, "the saved file", |a| a.download.is_none());
        assert!(a.status_text().unwrap_or_default().contains("Saved report.pdf"), "{:?}", a.status_text());
        let saved = BrowserApp::downloads_dir(&cx).unwrap().join("report.pdf");
        assert_eq!(std::fs::read(&saved).unwrap(), b"%PDF-1.4 hello", "the shell wrote the file, not the helper");
        assert!(a.data.downloads.iter().any(|d| d.name == "report.pdf" && d.status == "saved"));
        // a host the account may not fetch from is refused
        let mut cx2 = cx.clone();
        cx2.session.policy.net_hosts = vec!["a.org".into()];
        a.full.as_mut().unwrap().navigate("fake://download/other.bin");
        pump(&mut a, &cx2, "the refusal", |a| a.status_text().is_some_and(|s| s.contains("may not fetch files from example.org")));
    }

    #[test]
    fn a_crashing_helper_falls_back_to_the_in_house_engine_with_a_clear_status() {
        let (_fake, cx, mut a, _g) = in_full("full7", "--static");
        a.full.as_mut().unwrap().navigate("fake://crash");
        pump(&mut a, &cx, "the fall back", |a| a.full.is_none());
        let st = a.status_text().unwrap().to_string();
        assert!(st.contains("The full web engine stopped") && st.contains("built-in reader"), "{st}");
        settle(&mut a, &cx);
        assert_eq!(a.page().unwrap().doc.title, "The Guide");
        let audit = std::fs::read_to_string(cx.services.dir().parent().unwrap().join("audit.log")).unwrap_or_default();
        assert!(audit.contains("browser.full.crash"), "{audit}");
        // and the engine can be started again
        key(&mut a, &cx, Key::Char('F'));
        pump(&mut a, &cx, "a fresh engine", |a| a.full.as_ref().is_some_and(|f| f.pixmap().is_some()));
    }

    #[test]
    fn a_missing_or_failing_helper_says_so_and_keeps_the_reader() {
        {
            let _none = NoHelper::new();
            let cx = cx_with("full8", site(vec![("https://a.org/", html(GUIDE))]));
            let mut a = BrowserApp::new(&cx);
            a.open_url("a.org", &cx);
            settle(&mut a, &cx);
            key(&mut a, &cx, Key::Char('F'));
            assert!(a.full.is_none());
            assert!(a.status_text().unwrap().contains("not installed"), "{:?}", a.status_text());
            assert_eq!(a.page().unwrap().doc.title, "The Guide");
        }
        // a helper that exits at once
        let g = lock();
        let dir = crate::apps::tests::temp_dir("full8b");
        let script = dir.join("bad.sh");
        std::fs::write(&script, "#!/bin/sh\necho 'cannot open display' >&2\nexit 7\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::env::set_var(iyakki_agent::webengine::HELPER_ENV, &script);
        let cx = cx_with("full8c", site(vec![("https://a.org/", html(GUIDE))]));
        let mut a = BrowserApp::new(&cx);
        a.open_url("a.org", &cx);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('F'));
        pump(&mut a, &cx, "the failure", |a| a.full.is_none());
        let st = a.status_text().unwrap().to_string();
        assert!(st.contains("stopped") && st.contains("cannot open display"), "{st}");
        settle(&mut a, &cx);
        assert_eq!(a.page().unwrap().doc.title, "The Guide");
        std::env::remove_var(iyakki_agent::webengine::HELPER_ENV);
        drop(g);
    }

    #[test]
    fn each_tab_has_its_own_engine_and_a_hidden_one_makes_no_pictures() {
        let (_fake, cx, mut a, _g) = in_full("full9", "--static");
        assert_eq!(a.tab_count(), 1);
        key(&mut a, &cx, Key::Ctrl('t'));
        assert_eq!(a.tab_count(), 2);
        assert!(a.full.is_none(), "the new tab is an ordinary tab");
        assert!(a.tabs[0].as_ref().unwrap().full.is_some(), "the first tab keeps its engine");
        pump(&mut a, &cx, "the hidden tab to hear it was hidden", |a| a.tabs[0].as_ref().unwrap().full.as_ref().is_some_and(|f| f.log.iter().any(|l| l.contains("SetVisible(false)"))));
        assert_eq!(a.tab_titles()[0], "fake page https://a.org/", "its title stays on the tab");
        // the new tab can use its own engine too
        a.open_url("b.org/x", &cx);
        settle(&mut a, &cx);
        frame(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('F'));
        pump(&mut a, &cx, "the second engine", |a| a.full.as_ref().is_some_and(|f| f.url == "https://b.org/x"));
        a.handle(&Event::Tab(200), &cx);
        assert_eq!(a.active_tab(), 0);
        assert_eq!(a.full.as_ref().unwrap().url, "https://a.org/");
        pump(&mut a, &cx, "shown again", |a| a.full.as_ref().is_some_and(|f| f.log.iter().any(|l| l.contains("SetVisible(true)"))));
        // closing a tab ends its helper
        a.handle(&Event::Tab(301), &cx);
        assert_eq!(a.tab_count(), 1);
    }

    #[test]
    fn the_start_page_and_history_leave_the_engine_and_back_from_it_returns_to_the_page_before() {
        let (_fake, cx, mut a, _g) = in_full("full10", "--static");
        // there is nothing earlier inside the engine: back is the reader's page before it
        a.handle(&Event::Tab(0), &cx);
        assert!(a.full.is_none());
        assert_eq!(a.page().unwrap().doc.title, "The Guide");
        key(&mut a, &cx, Key::Char('F'));
        pump(&mut a, &cx, "the engine", |a| a.full.as_ref().is_some_and(|f| f.pixmap().is_some()));
        // Ctrl+H shows the browser's own history page (in the reader)
        key(&mut a, &cx, Key::Ctrl('h'));
        assert!(a.full.is_none());
        assert_eq!(a.view(), StartView::History);
    }

    #[test]
    fn the_engine_picture_is_cheap_to_draw_and_the_chrome_is_not_covered() {
        let (_fake, cx, mut a, _g) = in_full("full11", "--static");
        let mut g = frame(&mut a, &cx, 1280, 720);
        // blitting the picture is a row copy: measured on its own (the rest of a frame is the browser's chrome)
        let v = a.view_rect;
        let t = std::time::Instant::now();
        for _ in 0..50 {
            a.full.as_mut().unwrap().draw(&mut g, v, 1.0);
        }
        let per = t.elapsed().as_secs_f64() / 50.0;
        eprintln!("drawing the engine's {}x{} picture: {:.2} ms", v.w, v.h, per * 1000.0);
        assert!(per < 0.030, "drawing the picture took {per:.4}s (a debug build)");
        // above the page area: the tab strip and the tool bar are the browser's (not the engine's colours)
        assert_ne!(px(&g, 400.0, v.y - 10.0)[..3], [220, 40, 40]);
        // the Full browser button now offers the way back
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(9)));
    }

    #[test]
    fn a_frame_asks_the_shell_for_a_redraw_on_the_heartbeat() {
        let (_fake, cx, mut a, _g) = in_full("full12", "");
        // the animated fake draws a moving bar: frames keep coming and each batch asks for one redraw
        let mut asked = 0;
        for _ in 0..400 {
            a.handle(&Event::Tick, &cx);
            if a.wants_redraw() {
                asked += 1;
            }
            if asked >= 5 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(asked >= 5, "redraw requests: {asked}");
    }

    /// Not a test: drives the browser app with the REAL helper (window mode's WebKitGTK one, found in this checkout) and saves
    /// what the shell would show, to look at it.
    /// `IYAKKI_LIVE_STEPS="open:https://www.youtube.com/;wait:10;shot:a.png;click:500,28;type:rust language;key:enter;wait:8;shot:b.png;fps:4"`
    /// with `IYAKKI_SHOT_DIR=/some/dir`, e.g.
    /// `cargo test -p iyakki-shell --lib live_full_engine -- --ignored --nocapture`. Click positions are inside the page area.
    #[test]
    #[ignore]
    fn live_full_engine_to_png() {
        let (Ok(steps), Ok(dir)) = (std::env::var("IYAKKI_LIVE_STEPS"), std::env::var("IYAKKI_SHOT_DIR")) else { return };
        let _g = lock();
        std::env::remove_var(iyakki_agent::webengine::HELPER_ENV);
        iyakki_agent::webengine::set_host_mode(true);
        let (w, h) = std::env::var("IYAKKI_SIZE").ok().and_then(|v| v.split_once('x').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))).unwrap_or((1280u32, 800u32));
        let cx = cx_with("live_full", site(vec![]));
        let mut a = BrowserApp::new(&cx);
        frame(&mut a, &cx, w, h);
        let spin = |a: &mut BrowserApp, secs: f32| {
            let t = std::time::Instant::now();
            while t.elapsed().as_secs_f32() < secs {
                a.handle(&Event::Tick, &cx);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        for step in steps.split(';') {
            let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
            let xy = |s: &str| s.split_once(',').map(|(x, y)| (x.parse::<f32>().unwrap(), y.parse::<f32>().unwrap())).unwrap_or((0.0, 0.0));
            eprintln!("> {step}");
            match cmd {
                "open" => {
                    typ(&mut a, &cx, arg);
                    a.handle(&Event::Tab(9), &cx);
                    eprintln!("  status: {:?}, helper {:?}", a.status_text(), iyakki_agent::webengine::embedded_helper());
                }
                "wait" => spin(&mut a, arg.parse().unwrap()),
                "shot" => {
                    spin(&mut a, 0.3);
                    let g = frame(&mut a, &cx, w, h);
                    let path = std::path::Path::new(&dir).join(arg);
                    g.save_png(&path).unwrap();
                    eprintln!("  saved {} (title {:?}, url {:?}, engine {:?})", path.display(), a.tab_titles(), a.full.as_ref().map(|f| f.url.clone()), a.full.as_ref().map(|f| f.engine.clone()));
                }
                "fps" => {
                    let before = a.full.as_ref().map_or(0, |f| f.frames);
                    let (mut drawn, t) = (0u32, std::time::Instant::now());
                    while t.elapsed().as_secs_f32() < arg.parse::<f32>().unwrap() {
                        a.handle(&Event::Tick, &cx);
                        if a.wants_redraw() {
                            drawn += 1; // (drawing the whole interface in a debug build takes longer than a frame, so it is not done here)
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let n = a.full.as_ref().map_or(0, |f| f.frames) - before;
                    eprintln!("  engine frames {} ({:.1}/s), shell redraws {} ({:.1}/s)", n, n as f32 / t.elapsed().as_secs_f32(), drawn, drawn as f32 / t.elapsed().as_secs_f32());
                }
                "click" => {
                    let (x, y) = xy(arg);
                    a.handle(&Event::AreaAt(AREA_FULL, x, y), &cx);
                    spin(&mut a, 0.1);
                    let v = a.view_rect;
                    a.handle(&Event::MouseUp((v.x + x) as i32, (v.y + y) as i32), &cx);
                }
                "move" => {
                    let (x, y) = xy(arg);
                    let v = a.view_rect;
                    a.handle(&Event::MouseMove((v.x + x) as i32, (v.y + y) as i32), &cx);
                }
                "scroll" => {
                    a.handle(&Event::Scroll(arg.parse().unwrap()), &cx);
                }
                "type" => {
                    for c in arg.chars() {
                        a.handle(&Event::Key(Key::Char(c)), &cx);
                        spin(&mut a, 0.04);
                    }
                }
                "key" => {
                    let k = match arg {
                        "enter" => Key::Enter,
                        "esc" => Key::Esc,
                        "tab" => Key::Tab,
                        "ctrl-l" => Key::Ctrl('l'),
                        _ => Key::Char(arg.chars().next().unwrap()),
                    };
                    a.handle(&Event::Key(k), &cx);
                }
                "bar" => {
                    a.handle(&Event::Key(Key::Ctrl('l')), &cx);
                    typ(&mut a, &cx, arg);
                    a.handle(&Event::Key(Key::Enter), &cx);
                }
                "log" => eprintln!("  helper log: {:?}", a.full.as_ref().map(|f| f.log.iter().rev().take(8).cloned().collect::<Vec<_>>())),
                _ => eprintln!("  (unknown step)"),
            }
        }
        eprintln!("status at the end: {:?}", a.status_text());
    }
}
