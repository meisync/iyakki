//! Camera and microphone for pages in the full web engine (docs/development/BROWSER-MEDIA-DEVICES.md): the policy that decides a
//! request, the small per-person store of what the person said about a site, the question ("Allow meet.example to use your camera?")
//! and the indicator text. The helper only asks (`FromHelper::PermissionRequest`) and obeys the answer (`ToHelper::PermissionAnswer`);
//! it denies on silence. Nothing here touches a device: the engine opens it after an allow.
//!
//! Each kind is its own question and its own memory: microphone, camera and screen share. A request for several is asked one kind at
//! a time. Rules (the first that applies wins): the page must be a secure origin (https, or http on localhost); a tab in the
//! background never gets a device; a kind the person blocked stays blocked until they remove it in Settings; a kind they allowed
//! (always, or while the site is open) is allowed without asking; the device list alone (`enumerateDevices` names) is shown only to a
//! site that already has a device; everything else is a question. **Screen share is stricter**: it is asked every time, the
//! answers are "this time only", "Block this site" and "Not now", and an allow is never remembered (not for the session either):
//! the screen shows everything open on this computer. Audit lines name the host and the kind, never anything seen or heard.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iyakki_webview::proto::media_kind::{CAMERA, DEVICE_LIST, MICROPHONE, SCREEN};

use super::AppCx;
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, MUTED, R, TEXT};

/// Button ids (`Event::Tab`) in the browser.
pub const TAB_ONCE: usize = 17;
pub const TAB_WHILE_OPEN: usize = 18;
pub const TAB_ALWAYS: usize = 19;
pub const TAB_BLOCK: usize = 20;
pub const TAB_NOT_NOW: usize = 21;
/// The microphone chooser in the microphone question (a select box; the key M does the same).
pub const TAB_MIC: usize = 27;
/// The camera chooser in the camera question (a select box; the key C does the same).
pub const TAB_CAM: usize = 28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Say no without asking; the text is for the audit line.
    Deny(&'static str),
    /// Ask about this one kind (a `media_kind` bit); the others of the request are asked after it.
    Ask(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// This request only.
    Once,
    /// Until the person leaves the site (or closes the browser).
    WhileOpen,
    /// Remembered for this person.
    Always,
    /// Remembered no: the site is not asked again until the person removes it.
    Block,
    /// No, this time (Esc).
    NotNow,
}

/// Keys 1 to 4 (a screen share has only 1 = this time only and 4 = block); Esc is "not now".
pub fn answer_for_key(k: &Key, screen: bool) -> Option<Answer> {
    let a = match k {
        Key::Char('1') => Answer::Once,
        Key::Char('2') => Answer::WhileOpen,
        Key::Char('3') => Answer::Always,
        Key::Char('4') => Answer::Block,
        Key::Esc => Answer::NotNow,
        _ => return None,
    };
    (!screen || matches!(a, Answer::Once | Answer::Block | Answer::NotNow)).then_some(a)
}

pub fn answer_for_tab(n: usize) -> Option<Answer> {
    match n {
        TAB_ONCE => Some(Answer::Once),
        TAB_WHILE_OPEN => Some(Answer::WhileOpen),
        TAB_ALWAYS => Some(Answer::Always),
        TAB_BLOCK => Some(Answer::Block),
        TAB_NOT_NOW => Some(Answer::NotNow),
        _ => None,
    }
}

/// "camera", "microphone", "screen", "camera and microphone", "camera, microphone and screen".
pub fn kinds_words(kinds: u8) -> String {
    let mut w = Vec::new();
    if kinds & CAMERA != 0 {
        w.push("camera");
    }
    if kinds & MICROPHONE != 0 {
        w.push("microphone");
    }
    if kinds & SCREEN != 0 {
        w.push("screen");
    }
    match w.len() {
        0 => "devices".into(),
        1 => w[0].into(),
        n => format!("{} and {}", w[..n - 1].join(", "), w[n - 1]),
    }
}

/// The text of the indicator while a page captures ("Camera on", "Microphone + screen on", ...), `None` when nothing is.
pub fn indicator(capturing: u8) -> Option<String> {
    let k = capturing & (CAMERA | MICROPHONE | SCREEN);
    if k == 0 {
        return None;
    }
    let mut w = Vec::new();
    for (bit, word) in [(CAMERA, "Camera"), (MICROPHONE, "Microphone"), (SCREEN, "Screen")] {
        if k & bit != 0 {
            w.push(if w.is_empty() { word.to_string() } else { word.to_lowercase() });
        }
    }
    Some(format!("{} on", w.join(" + ")))
}

/// What is capturing now, for the sign the shell draws on every screen (set by the browser every beat).
static CAPTURE_NOW: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// The person clicked the sign: the browser stops every capture at its next beat.
static STOP_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The last non-empty capture and when it was seen: the sign stays 2 s after the page lets go, so a page that
/// restarts its camera (a retry) does not make the sign flicker.
static LAST_ON: std::sync::Mutex<(u8, Option<std::time::Instant>)> = std::sync::Mutex::new((0, None));
const SIGN_HOLD: std::time::Duration = std::time::Duration::from_secs(2);

pub fn publish_capture(bits: u8) {
    let bits = bits & (CAMERA | MICROPHONE | SCREEN);
    CAPTURE_NOW.store(bits, std::sync::atomic::Ordering::Relaxed);
    if bits != 0 {
        if let Ok(mut l) = LAST_ON.lock() {
            *l = (bits, Some(std::time::Instant::now()));
        }
    }
}

/// What the sign shows: what is capturing now, or what was a moment ago.
pub fn capture_now() -> u8 {
    let now = CAPTURE_NOW.load(std::sync::atomic::Ordering::Relaxed);
    if now != 0 {
        return now;
    }
    match LAST_ON.lock() {
        Ok(l) => match l.1 {
            Some(t) if t.elapsed() < SIGN_HOLD => l.0,
            _ => 0,
        },
        Err(_) => 0,
    }
}

/// The shell's click on the sign.
pub fn ask_stop() {
    STOP_ASKED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn take_stop() -> bool {
    STOP_ASKED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// The host of a secure origin: https, or http on the machine itself. `None` for anything else (a plain http site, a file, data:).
pub fn secure_host(origin: &str) -> Option<String> {
    let u = iyakki_web::Url::parse(origin).ok()?;
    let host = u.host.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let local = host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host.ends_with(".localhost");
    (u.https || local).then_some(host)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Site {
    allow: u8,
    block: u8,
}

/// What this person has said about sites, plus the "while this site is open" allowances (memory only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaPerms {
    sites: BTreeMap<String, Site>,
    session: BTreeMap<String, u8>,
    /// The camera and the microphone a site was given with "Always allow this site" (`host -> (camera id, microphone id)`; empty: none).
    devices: BTreeMap<String, (String, String)>,
}

const CAPTURE: u8 = CAMERA | MICROPHONE;
const ALL: u8 = CAMERA | MICROPHONE | SCREEN;

impl MediaPerms {
    /// The decision for a request from `origin` (the page address) for `kinds`; `foreground` is whether the tab is the one shown;
    /// `granted` are the kinds the person already said "this time only" to for this same request.
    pub fn decide(&self, origin: &str, kinds: u8, foreground: bool, granted: u8) -> Decision {
        let Some(host) = secure_host(origin) else { return Decision::Deny("not a secure origin") };
        if !foreground {
            return Decision::Deny("tab in the background");
        }
        let site = self.sites.get(&host).copied().unwrap_or_default();
        let have = site.allow | self.session.get(&host).copied().unwrap_or(0);
        if kinds & DEVICE_LIST != 0 {
            return if have & CAPTURE != 0 { Decision::Allow } else { Decision::Deny("no device allowed for this site") };
        }
        let want = kinds & ALL;
        if want == 0 {
            return Decision::Deny("nothing asked for");
        }
        if site.block & want != 0 {
            return Decision::Deny("blocked by the person");
        }
        // the screen is never remembered: only this request's own "this time only" counts
        let ok = (have & CAPTURE) | granted;
        match [MICROPHONE, CAMERA, SCREEN].into_iter().find(|k| want & k != 0 && ok & k == 0) {
            None => Decision::Allow,
            Some(k) => Decision::Ask(k),
        }
    }

    /// Record the person's answer about ONE kind for `origin`; returns whether it is a yes. A screen share is never remembered as an
    /// allow (the answers "while open" and "always" count as "this time only" for it).
    pub fn apply(&mut self, origin: &str, kind: u8, a: Answer) -> bool {
        let Some(host) = secure_host(origin) else { return false };
        let kind = kind & ALL;
        let screen = kind == SCREEN;
        match a {
            Answer::Once => true,
            Answer::WhileOpen if !screen => {
                *self.session.entry(host).or_insert(0) |= kind;
                true
            }
            Answer::Always if !screen => {
                let s = self.sites.entry(host).or_default();
                s.allow |= kind;
                s.block &= !kind;
                true
            }
            Answer::WhileOpen | Answer::Always => true,
            Answer::Block => {
                let s = self.sites.entry(host).or_default();
                s.block |= kind;
                s.allow &= !kind;
                false
            }
            Answer::NotNow => false,
        }
    }

    /// The person moved to another site (or closed the tab): "while this site is open" ends for every other host.
    pub fn page_moved(&mut self, host: &str) {
        self.session.retain(|h, _| h == host);
    }

    pub fn end_session(&mut self) {
        self.session.clear();
    }

    /// Forget what was remembered about a site (Settings -> Browser): the next request asks again.
    pub fn revoke(&mut self, host: &str) {
        let h = host.to_ascii_lowercase();
        self.sites.remove(&h);
        self.session.remove(&h);
        self.devices.remove(&h);
    }

    /// The device of `kind` (CAMERA or MICROPHONE) this site was given with "Always allow", or "" when none.
    pub fn site_device(&self, host: &str, kind: u8) -> String {
        self.devices.get(&host.to_ascii_lowercase()).map(|d| if kind == CAMERA { d.0.clone() } else if kind == MICROPHONE { d.1.clone() } else { String::new() }).unwrap_or_default()
    }

    /// Remember that this site gets device `id` of `kind` (only for a site that is allowed always; an empty id forgets it).
    pub fn set_site_device(&mut self, host: &str, kind: u8, id: &str) {
        let h = host.to_ascii_lowercase();
        if self.sites.get(&h).map_or(true, |s| s.allow & kind == 0) || id.len() > 120 || id.chars().any(|c| c.is_control()) {
            return;
        }
        let e = self.devices.entry(h.clone()).or_default();
        if kind == CAMERA {
            e.0 = id.to_string();
        } else if kind == MICROPHONE {
            e.1 = id.to_string();
        }
        if e.0.is_empty() && e.1.is_empty() {
            self.devices.remove(&h);
        }
    }

    /// The remembered sites for the Settings list: host and a sentence ("camera allowed", "microphone blocked").
    pub fn list(&self) -> Vec<(String, String)> {
        self.sites
            .iter()
            .filter(|(_, s)| s.allow | s.block != 0)
            .map(|(h, s)| {
                let mut parts = Vec::new();
                for (bits, word) in [(s.allow, "allowed"), (s.block, "blocked")] {
                    if bits & ALL != 0 {
                        parts.push(format!("{} {word}", kinds_words(bits & ALL)));
                    }
                }
                (h.clone(), parts.join(", "))
            })
            .collect()
    }

    fn to_json(&self) -> serde_json::Value {
        let sites: serde_json::Map<String, serde_json::Value> = self.sites.iter().filter(|(_, s)| s.allow | s.block != 0).map(|(h, s)| (h.clone(), serde_json::json!({ "allow": s.allow, "block": s.block }))).collect();
        let devices: serde_json::Map<String, serde_json::Value> = self.devices.iter().filter(|(h, d)| self.sites.contains_key(*h) && !(d.0.is_empty() && d.1.is_empty())).map(|(h, d)| (h.clone(), serde_json::json!({ "cam": d.0, "mic": d.1 }))).collect();
        serde_json::json!({ "sites": sites, "devices": devices })
    }

    fn from_json(text: &str) -> MediaPerms {
        let mut m = MediaPerms::default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return m };
        for (h, s) in v["sites"].as_object().into_iter().flatten().take(500) {
            let ok = !h.is_empty() && h.len() < 254 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '[' | ']' | ':'));
            if ok {
                let bits = |k: &str, mask: u8| (s[k].as_u64().unwrap_or(0) as u8) & mask;
                // an allow is only ever kept for the camera and the microphone; a screen share is never remembered as an allow
                m.sites.insert(h.to_ascii_lowercase(), Site { allow: bits("allow", CAPTURE) & !bits("block", ALL), block: bits("block", ALL) });
            }
        }
        for (h, d) in v["devices"].as_object().into_iter().flatten().take(500) {
            let h = h.to_ascii_lowercase();
            let ok = |x: &serde_json::Value| x.as_str().filter(|t| t.len() <= 120 && !t.chars().any(|c| c.is_control())).unwrap_or("").to_string();
            if m.sites.get(&h).is_some_and(|s| s.allow != 0) {
                m.devices.insert(h, (ok(&d["cam"]), ok(&d["mic"])));
            }
        }
        m
    }
}

fn path(cx: &AppCx) -> PathBuf {
    cx.services.dir().join(format!("browser-media-{}.json", super::browser_config::who(cx)))
}

/// Where this person's device choices are kept (`iyakki_agent::browserdev::DevicePrefs`): the default camera and the hidden devices.
pub fn devprefs_path(cx: &AppCx) -> PathBuf {
    cx.services.dir().join(format!("browser-devices-{}.json", super::browser_config::who(cx)))
}

/// This person's remembered choices (nothing remembered when the file is missing or damaged).
pub fn load(cx: &AppCx) -> MediaPerms {
    super::browser_config::read_checked("browser media choices", &path(cx)).map(|t| MediaPerms::from_json(&t)).unwrap_or_default()
}

pub fn save(cx: &AppCx, m: &MediaPerms) {
    super::browser_config::write_checked("browser media choices", &path(cx), &m.to_json().to_string());
}

/// The audit line: host, kind and what happened. Never a picture, a sound or a device name.
pub fn audit(cx: &AppCx, action: &str, origin: &str, kinds: u8, outcome: &str) {
    let host = secure_host(origin).or_else(|| iyakki_sec::url_host(origin)).unwrap_or_default();
    cx.audit.record(&cx.session.user, action, &format!("{host} {} {outcome}", kinds_words(kinds)));
}


// ------------------------------------------------------------------------------------------------ screen share frames

/// The file a granted screen share is fed from: the shell writes its own screen into it (about 4 times a second, only while a share is
/// on) and the engine side reads it. 32-byte header, then `h` rows of `stride` bytes of RGBA:
/// `"IYSC"`, `u32 version (1)`, `u32 width`, `u32 height`, `u32 stride`, `u32 format (0 = RGBA)`, `u64 seq`, all little-endian.
/// `seq` is odd while a picture is being written and even when it is complete: a reader takes a picture only when it reads the same
/// even `seq` before and after copying. (docs/development/BROWSER-MEDIA-DEVICES.md, "Screen share")
pub struct ScreenFeed {
    file: std::fs::File,
    path: PathBuf,
    seq: u64,
}

pub const FEED_HEADER: usize = 32;

impl ScreenFeed {
    pub fn create(path: PathBuf) -> std::io::Result<ScreenFeed> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).mode(0o600).open(&path)?;
        Ok(ScreenFeed { file, path, seq: 0 })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Write one picture (`rgba` is `w * h * 4` bytes). A wrong size is refused.
    pub fn write(&mut self, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;
        if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "picture size does not match"));
        }
        let mut head = [0u8; FEED_HEADER];
        head[0..4].copy_from_slice(b"IYSC");
        head[4..8].copy_from_slice(&1u32.to_le_bytes());
        head[8..12].copy_from_slice(&w.to_le_bytes());
        head[12..16].copy_from_slice(&h.to_le_bytes());
        head[16..20].copy_from_slice(&(w * 4).to_le_bytes());
        self.seq += 1;
        head[24..32].copy_from_slice(&self.seq.to_le_bytes()); // odd: being written
        self.file.write_all_at(&head, 0)?;
        self.file.write_all_at(rgba, FEED_HEADER as u64)?;
        self.seq += 1;
        self.file.write_all_at(&self.seq.to_le_bytes(), 24)?; // even: complete
        Ok(())
    }
}

impl Drop for ScreenFeed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Read the header of a feed file: `(width, height, seq)`; `None` when it is not a feed file.
pub fn read_feed_header(path: &std::path::Path) -> Option<(u32, u32, u64)> {
    let b = std::fs::read(path).ok()?;
    if b.len() < FEED_HEADER || &b[0..4] != b"IYSC" {
        return None;
    }
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    Some((u(8), u(12), u64::from_le_bytes(b[24..32].try_into().unwrap())))
}

/// Where the shell should write its screen now (`Some` only while a screen share is on).
static FEED_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

pub fn set_feed_path(p: Option<PathBuf>) {
    if let Ok(mut g) = FEED_PATH.lock() {
        *g = p;
    }
}

pub fn feed_path() -> Option<PathBuf> {
    FEED_PATH.lock().ok().and_then(|g| g.clone())
}

/// A fresh file name for this process's share: in /dev/shm where it exists, else the temp folder.
pub fn new_feed_path() -> PathBuf {
    let dir = iyakki_agent::sysinfo::shared_memory_dir();
    dir.join(format!("iyakki-screen-{}-{}.raw", std::process::id(), FEED_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)))
}

static FEED: std::sync::Mutex<Option<(ScreenFeed, std::time::Instant)>> = std::sync::Mutex::new(None);

/// Called after every picture the shell puts on its screen: while a screen share is on (`set_feed_path`), writes the screen into the feed
/// file at most 4 times a second; when none is on, closes the feed (and removes the file). Cheap when idle: one lock.
pub fn feed_screen(w: u32, h: u32, rgba: &[u8]) {
    let want = feed_path();
    let Ok(mut g) = FEED.lock() else { return };
    match (want, g.as_mut()) {
        (None, _) => *g = None,
        (Some(p), cur) if cur.as_ref().map_or(true, |(f, _)| f.path() != p) => {
            *g = ScreenFeed::create(p).ok().map(|f| (f, std::time::Instant::now() - std::time::Duration::from_secs(1)));
        }
        _ => {}
    }
    if let Some((f, last)) = g.as_mut() {
        if last.elapsed() >= std::time::Duration::from_millis(250) && f.write(w, h, rgba).is_ok() {
            *last = std::time::Instant::now();
        }
    }
}

static FEED_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A question on the screen: one kind of one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: u32,
    /// Everything the request wants.
    pub kinds: u8,
    /// The one kind this question is about.
    pub asking: u8,
    /// The kinds already answered "this time only" in this request.
    pub granted: u8,
    pub origin: String,
    pub host: String,
}

impl Ask {
    pub fn new(id: u32, kinds: u8, asking: u8, granted: u8, origin: &str) -> Ask {
        Ask { id, kinds, asking, granted, origin: origin.to_string(), host: secure_host(origin).unwrap_or_default() }
    }

    pub fn is_screen(&self) -> bool {
        self.asking == SCREEN
    }

    pub fn question(&self) -> String {
        if self.is_screen() {
            format!("Allow {} to share your screen?", self.host)
        } else {
            format!("Allow {} to use your {}?", self.host, kinds_words(self.asking))
        }
    }
}

/// Draw the question over `area`; the buttons send the `Event::Tab(TAB_*)` ids.
/// `device` is the line of the chooser for the kind asked (`Some` for a camera or microphone question); `device_open` is whether its list is open.
pub fn render(g: &mut Gfx, area: R, s: f32, ask: &Ask, device: Option<&str>, device_open: bool) {
    let mic = device;
    let mic_open = device_open;
    let accent = if ask.is_screen() { crate::gfx::rgb(255, 110, 90) } else { crate::gfx::rgb(255, 170, 70) };
    // the microphone question has one more row: which microphone (the same choice as the Voice tile)
    let extra = if mic.is_some() { 48.0 * s } else { 0.0 };
    let p = crate::fx::modal(g, area, s, 640.0 * s, 250.0 * s + extra, accent, ask.is_screen());
    for (i, l) in g.wrap(18.0 * s, &ask.question(), Face::Bold, p.w - 48.0 * s).into_iter().take(2).enumerate() {
        g.text(p.x + 24.0 * s, p.y + 24.0 * s + i as f32 * 26.0 * s, 18.0 * s, TEXT, &l, Face::Bold);
    }
    let note = if ask.is_screen() {
        "The page will see everything on this screen, in every tile, until you stop it. It is asked every time: nothing is remembered except Block."
    } else {
        "The page can then see and hear through it until you leave the site or block it. You can change this later in Settings."
    };
    for (i, l) in g.wrap(14.0 * s, note, Face::Sans, p.w - 48.0 * s).into_iter().take(3).enumerate() {
        g.text(p.x + 24.0 * s, p.y + 82.0 * s + i as f32 * 20.0 * s, 14.0 * s, MUTED, &l, Face::Sans);
    }
    if let Some(label) = mic {
        let y = p.y + 150.0 * s;
        let (lab, tab, what) = if ask.asking == CAMERA { ("Camera  (C)", TAB_CAM, "camera") } else { ("Microphone  (M)", TAB_MIC, "microphone") };
        g.text(p.x + 24.0 * s, y + 6.0 * s, 14.0 * s, MUTED, lab, Face::Sans);
        let lx = p.x + 24.0 * s + g.text_width(14.0 * s, lab, Face::Sans) + 12.0 * s;
        let w = (p.x + p.w - 24.0 * s - lx).max(120.0 * s);
        super::widgets::select_box(g, lx, y, w, s, label, mic_open, accent, Event::Tab(tab), what);
    }
    let grey = crate::gfx::rgb(150, 160, 180);
    let (mut x, y) = (p.x + 24.0 * s, p.y + p.h - 90.0 * s);
    if ask.is_screen() {
        g.button(x, y, 14.0 * s, "1  Share this time only", accent, Event::Tab(TAB_ONCE));
        let y = y + 40.0 * s;
        let mut x = p.x + 24.0 * s;
        x += g.button(x, y, 14.0 * s, "4  Block this site", crate::ui::ERR, Event::Tab(TAB_BLOCK)) + 10.0 * s;
        g.button(x, y, 14.0 * s, "Not now (Esc)", grey, Event::Tab(TAB_NOT_NOW));
        return;
    }
    x += g.button(x, y, 14.0 * s, "1  Allow once", accent, Event::Tab(TAB_ONCE)) + 10.0 * s;
    g.button(x, y, 14.0 * s, "2  Allow while this site is open", accent, Event::Tab(TAB_WHILE_OPEN));
    let y = y + 40.0 * s;
    let mut x = p.x + 24.0 * s;
    x += g.button(x, y, 14.0 * s, "3  Always allow this site", grey, Event::Tab(TAB_ALWAYS)) + 10.0 * s;
    x += g.button(x, y, 14.0 * s, "4  Block this site", crate::ui::ERR, Event::Tab(TAB_BLOCK)) + 10.0 * s;
    g.button(x, y, 14.0 * s, "Not now (Esc)", grey, Event::Tab(TAB_NOT_NOW));
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEET: &str = "https://meet.example/room";

    #[test]
    fn a_site_remembers_its_devices_only_when_always_allowed_and_forgetting_the_site_forgets_them() {
        let mut m = MediaPerms::default();
        m.set_site_device("meet.example", CAMERA, "USB Cam: x");
        assert_eq!(m.site_device("meet.example", CAMERA), "", "a site that is not allowed always remembers nothing");
        m.apply(MEET, CAMERA, Answer::Always);
        m.apply(MEET, MICROPHONE, Answer::Once);
        m.set_site_device("meet.example", CAMERA, "USB Cam: x");
        m.set_site_device("meet.example", MICROPHONE, "plughw:1,0");
        assert_eq!(m.site_device("MEET.example", CAMERA), "USB Cam: x", "host names are case-insensitive");
        assert_eq!(m.site_device("meet.example", MICROPHONE), "", "a microphone that was allowed only once is not remembered");
        assert_eq!(m.site_device("meet.example", SCREEN), "");
        let back = MediaPerms::from_json(&m.to_json().to_string());
        assert_eq!(back.site_device("meet.example", CAMERA), "USB Cam: x", "saved and reloaded");
        // a device name with control characters or a huge one is refused
        let mut n = MediaPerms::default();
        n.apply(MEET, CAMERA, Answer::Always);
        n.set_site_device("meet.example", CAMERA, "bad\u{7}name");
        n.set_site_device("meet.example", CAMERA, &"x".repeat(500));
        assert_eq!(n.site_device("meet.example", CAMERA), "");
        // a block takes the allow, and the device with it, when the file is read again
        m.apply(MEET, CAMERA, Answer::Block);
        assert_eq!(MediaPerms::from_json(&m.to_json().to_string()).site_device("meet.example", CAMERA), "");
        m.apply(MEET, CAMERA, Answer::Always);
        m.set_site_device("meet.example", CAMERA, "USB Cam: x");
        m.revoke("meet.example");
        assert_eq!(m.site_device("meet.example", CAMERA), "");
    }

    #[test]
    fn insecure_and_background_requests_are_denied_without_a_question() {
        let m = MediaPerms::default();
        assert!(matches!(m.decide("http://meet.example/", MICROPHONE, true, 0), Decision::Deny(_)));
        assert!(matches!(m.decide("file:///tmp/a.html", CAMERA, true, 0), Decision::Deny(_)));
        assert!(matches!(m.decide("about:blank", CAMERA, true, 0), Decision::Deny(_)));
        assert!(matches!(m.decide(MEET, CAMERA, false, 0), Decision::Deny(_)), "a background tab never gets a device");
        assert!(matches!(m.decide(MEET, SCREEN, false, 0), Decision::Deny(_)));
        assert_eq!(m.decide("http://localhost:8080/", CAMERA, true, 0), Decision::Ask(CAMERA), "the machine itself counts as secure");
        assert!(matches!(m.decide(MEET, 0, true, 0), Decision::Deny(_)));
    }

    #[test]
    fn each_kind_is_asked_by_itself_in_a_fixed_order() {
        let m = MediaPerms::default();
        let all = MICROPHONE | CAMERA | SCREEN;
        assert_eq!(m.decide(MEET, all, true, 0), Decision::Ask(MICROPHONE));
        assert_eq!(m.decide(MEET, all, true, MICROPHONE), Decision::Ask(CAMERA));
        assert_eq!(m.decide(MEET, all, true, MICROPHONE | CAMERA), Decision::Ask(SCREEN));
        assert_eq!(m.decide(MEET, all, true, all), Decision::Allow);
        assert_eq!(m.decide(MEET, CAMERA | MICROPHONE, true, MICROPHONE | CAMERA), Decision::Allow);
    }

    #[test]
    fn answers_are_remembered_as_the_person_chose() {
        let mut m = MediaPerms::default();
        assert!(m.apply(MEET, CAMERA, Answer::Once));
        assert_eq!(m.decide(MEET, CAMERA, true, 0), Decision::Ask(CAMERA), "once does not remember");
        assert!(m.apply(MEET, CAMERA, Answer::WhileOpen));
        assert_eq!(m.decide(MEET, CAMERA, true, 0), Decision::Allow);
        assert_eq!(m.decide(MEET, CAMERA | MICROPHONE, true, 0), Decision::Ask(MICROPHONE), "only the device that was allowed");
        assert_eq!(m.decide(MEET, DEVICE_LIST, true, 0), Decision::Allow, "names are shown to a site that has a device");
        m.page_moved("other.example");
        assert_eq!(m.decide(MEET, CAMERA, true, 0), Decision::Ask(CAMERA), "leaving the site ends 'while open'");
        assert!(matches!(m.decide(MEET, DEVICE_LIST, true, 0), Decision::Deny(_)));
        assert!(m.apply(MEET, MICROPHONE, Answer::Always));
        m.page_moved("other.example");
        assert_eq!(m.decide(MEET, MICROPHONE, true, 0), Decision::Allow);
        assert!(!m.apply(MEET, MICROPHONE, Answer::Block));
        assert!(matches!(m.decide(MEET, MICROPHONE, true, 0), Decision::Deny("blocked by the person")));
        assert!(matches!(m.decide(MEET, CAMERA | MICROPHONE, true, 0), Decision::Deny(_)));
        assert!(!m.apply(MEET, CAMERA, Answer::NotNow));
        assert_eq!(m.decide(MEET, CAMERA, true, 0), Decision::Ask(CAMERA), "not now remembers nothing");
        m.revoke("MEET.example");
        assert_eq!(m.decide(MEET, MICROPHONE, true, 0), Decision::Ask(MICROPHONE), "removing a site in Settings makes it ask again");
        assert!(m.list().is_empty());
    }

    #[test]
    fn a_screen_share_is_asked_every_time_and_only_block_is_remembered() {
        let mut m = MediaPerms::default();
        assert_eq!(m.decide(MEET, SCREEN, true, 0), Decision::Ask(SCREEN));
        // even if somebody offered 'always' or 'while open', nothing is kept
        assert!(m.apply(MEET, SCREEN, Answer::Always));
        assert!(m.apply(MEET, SCREEN, Answer::WhileOpen));
        assert!(m.apply(MEET, SCREEN, Answer::Once));
        assert_eq!(m.decide(MEET, SCREEN, true, 0), Decision::Ask(SCREEN));
        assert!(m.list().is_empty());
        // a camera allowed for ever does not make the screen allowed
        m.apply(MEET, CAMERA, Answer::Always);
        assert_eq!(m.decide(MEET, CAMERA | SCREEN, true, 0), Decision::Ask(SCREEN));
        // and the screen's own block is remembered, apart from the camera's allow
        assert!(!m.apply(MEET, SCREEN, Answer::Block));
        assert!(matches!(m.decide(MEET, SCREEN, true, 0), Decision::Deny("blocked by the person")));
        assert_eq!(m.decide(MEET, CAMERA, true, 0), Decision::Allow);
        assert_eq!(m.list(), vec![("meet.example".to_string(), "camera allowed, screen blocked".to_string())]);
        // a file that claims a remembered screen allow is read as no allow
        let back = MediaPerms::from_json(r#"{"sites":{"x.example":{"allow":7,"block":0}}}"#);
        assert_eq!(back.decide("https://x.example/", SCREEN, true, 0), Decision::Ask(SCREEN));
        assert_eq!(back.list(), vec![("x.example".to_string(), "camera and microphone allowed".to_string())]);
    }

    #[test]
    fn the_store_round_trips_ignores_junk_and_drops_session_allowances() {
        let mut m = MediaPerms::default();
        m.apply(MEET, CAMERA, Answer::Always);
        m.apply("https://b.example/", MICROPHONE, Answer::Block);
        m.apply("https://c.example/", CAMERA, Answer::WhileOpen);
        let back = MediaPerms::from_json(&m.to_json().to_string());
        assert_eq!(back.list(), vec![("b.example".to_string(), "microphone blocked".to_string()), ("meet.example".to_string(), "camera allowed".to_string())]);
        assert_eq!(back.decide("https://c.example/", CAMERA, true, 0), Decision::Ask(CAMERA), "session allowances are not saved");
        let junk = MediaPerms::from_json(r#"{"sites":{"bad host!":{"allow":3},"ok.example":{"allow":3,"block":1},"":{"allow":1}}}"#);
        assert_eq!(junk.list(), vec![("ok.example".to_string(), "camera allowed, microphone blocked".to_string())], "a bit that is both allowed and blocked is blocked");
        assert_eq!(MediaPerms::from_json("not json"), MediaPerms::default());
    }

    #[test]
    fn the_screen_feed_writes_a_complete_picture_with_an_even_sequence_and_removes_its_file() {
        let path = new_feed_path();
        let mut f = ScreenFeed::create(path.clone()).unwrap();
        let px: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        f.write(4, 3, &px).unwrap();
        assert_eq!(read_feed_header(&path), Some((4, 3, 2)));
        f.write(4, 3, &px).unwrap();
        assert_eq!(read_feed_header(&path), Some((4, 3, 4)), "two writes per picture: odd while writing, even when done");
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(&raw[FEED_HEADER..], &px[..]);
        assert!(f.write(4, 3, &px[1..]).is_err() && f.write(0, 3, &[]).is_err());
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);
        drop(f);
        assert!(!path.exists());
    }

    #[test]
    fn keys_buttons_words_and_the_indicator() {
        for (k, a) in [('1', Answer::Once), ('2', Answer::WhileOpen), ('3', Answer::Always), ('4', Answer::Block)] {
            assert_eq!(answer_for_key(&Key::Char(k), false), Some(a));
        }
        assert_eq!(answer_for_key(&Key::Esc, false), Some(Answer::NotNow));
        assert_eq!(answer_for_key(&Key::Char('y'), false), None);
        assert_eq!([Key::Char('1'), Key::Char('2'), Key::Char('3'), Key::Char('4'), Key::Esc].map(|k| answer_for_key(&k, true)), [Some(Answer::Once), None, None, Some(Answer::Block), Some(Answer::NotNow)], "no 'while open' or 'always' for the screen");
        assert_eq!([TAB_ONCE, TAB_WHILE_OPEN, TAB_ALWAYS, TAB_BLOCK, TAB_NOT_NOW].map(answer_for_tab), [Some(Answer::Once), Some(Answer::WhileOpen), Some(Answer::Always), Some(Answer::Block), Some(Answer::NotNow)]);
        assert_eq!(kinds_words(CAMERA | MICROPHONE | SCREEN), "camera, microphone and screen");
        assert_eq!(kinds_words(SCREEN), "screen");
        let ask = Ask::new(4, CAMERA | MICROPHONE, CAMERA, 0, MEET);
        assert_eq!(ask.question(), "Allow meet.example to use your camera?");
        let mut g = Gfx::new(1000, 700);
        render(&mut g, R::new(0.0, 0.0, 1000.0, 700.0), 1.0, &ask, None, false);
        for t in [TAB_ONCE, TAB_WHILE_OPEN, TAB_ALWAYS, TAB_BLOCK, TAB_NOT_NOW] {
            assert!(g.hits.iter().any(|h| h.ev == Event::Tab(t)), "button {t}");
        }
        let shot = Ask::new(5, SCREEN, SCREEN, 0, MEET);
        assert_eq!(shot.question(), "Allow meet.example to share your screen?");
        let mut g = Gfx::new(1000, 700);
        render(&mut g, R::new(0.0, 0.0, 1000.0, 700.0), 1.0, &shot, None, false);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(TAB_ONCE)) && g.hits.iter().any(|h| h.ev == Event::Tab(TAB_BLOCK)) && g.hits.iter().any(|h| h.ev == Event::Tab(TAB_NOT_NOW)));
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(TAB_ALWAYS) || h.ev == Event::Tab(TAB_WHILE_OPEN)), "the screen offers neither 'always' nor 'while open'");
        assert_eq!(indicator(CAMERA).as_deref(), Some("Camera on"));
        assert_eq!(indicator(CAMERA | MICROPHONE).as_deref(), Some("Camera + microphone on"));
        assert_eq!(indicator(SCREEN).as_deref(), Some("Screen on"));
        assert_eq!(indicator(MICROPHONE | SCREEN).as_deref(), Some("Microphone + screen on"));
        assert_eq!(indicator(0), None);
    }
}
