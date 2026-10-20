//! The IYAKKI menu: a click on the "IYAKKI" name in the launcher's top bar (or F10 anywhere after sign-in) opens a dropdown in
//! the design system's glass look with four entries, each a page of the same panel:
//!
//! * **About IYAKKI**: version, build, variant, kernel, IYAKKI's licence (Apache-2.0) and a scrollable list of the licences of
//!   everything inside (the Rust crates and the programs of the system images; [`crate::about`]).
//! * **Quick install**: k3s, and DuckDB and Elasticsearch on top of it, in dependency order, as detached jobs
//!   ([`iyakki_agent::quickinstall`]); "Installer runs" opens the top bar's installer list.
//! * **Media player**: which player this image has (ffplay, mpv, VLC, mplayer; MP3 plays in IYAKKI itself), a file to play, Stop,
//!   and MEDIA for the library.
//! * **Mac drivers**: Wi-Fi firmware (the script for macOS, then "Install from folder or USB"), T2 audio, Bluetooth and the USB
//!   camera, as the Devices popover reads them, and STORE → Drivers.
//!
//! Keys: Up/Down and Enter, the underlined letters, Esc goes back a page and closes on the first page, F10 closes. A click
//! outside the panel closes it. The menu only says what it wants ([`Outcome`]); the shell does it (jobs, the player, jumps).

use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_agent::quickinstall::{self, Item, Latest, Presence};

pub const BASE: usize = 1_200_000;
pub const CLOSE: usize = BASE;
pub const BACK: usize = BASE + 1;
/// A menu entry: `ENTRY + index`.
pub const ENTRY: usize = BASE + 10;
/// A Quick install row: `IROW + index`.
pub const IROW: usize = BASE + 100;
pub const INSTALL_BTN: usize = BASE + 30;
pub const RUNS_BTN: usize = BASE + 31;
pub const PLAY: usize = BASE + 40;
pub const STOP: usize = BASE + 41;
pub const OPEN_MEDIA: usize = BASE + 42;
pub const PATH_FIELD: usize = BASE + 43;
/// Starts the Quick install job of the media player (ffmpeg) from the Media page.
pub const INSTALL_PLAYER: usize = BASE + 44;
pub const DRIVERS: usize = BASE + 50;
pub const DEVICES: usize = BASE + 51;
pub const SCROLL: usize = BASE + 60;

const ACCENT: [u8; 4] = crate::fx::CYAN;
const AMBER: [u8; 4] = rgb(255, 196, 94);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Menu,
    About,
    Install,
    Media,
    Mac,
    /// Not a page: the entry opens Settings.
    Settings,
    /// Not a page: Settings → Developer mode (administrators only).
    Developer,
}

/// The entries of the first page: page, title, what it is, the key that opens it.
pub const ENTRIES: [(Page, &str, &str, char); 6] = [
    (Page::About, "About IYAKKI", "Version, build, kernel, and the licences of everything inside", 'a'),
    (Page::Install, "Quick install", "Cluster, AI coding agents, voices and local model servers, as background jobs", 'q'),
    (Page::Media, "Media player", "Play a video or audio file", 'm'),
    (Page::Mac, "Mac drivers", "Wi-Fi firmware, T2 audio, Bluetooth and the USB camera", 'd'),
    (Page::Settings, "Settings", "Network, sound, display, time, models, accounts and more", 's'),
    (Page::Developer, "Developer mode", "Let a paired computer install, run and watch apps here (administrators)", 'v'),
];

/// What a key or click asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Close,
    /// A page opened that needs facts only the shell has (Install: what is installed; Mac: the devices; Media: the players).
    Opened(Page),
    Install(Item),
    /// Open the installer list of the top bar.
    ShowInstalls,
    Play(String),
    StopMedia,
    /// Open an app at an item (`("store", "drivers")`, `("media", "")`).
    Jump(&'static str, String),
}

/// The media players looked for, in the order MEDIA uses them, with the arguments that keep them quiet.
pub const PLAYERS: [&str; 5] = ["ffmpeg", "ffplay", "mpv", "vlc", "mplayer"];

/// Which players a machine has: `(name, path)` for each found by `lookup` (a program name to its path).
pub fn players_found(lookup: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    PLAYERS.iter().filter_map(|p| lookup(p).map(|path| (p.to_string(), path))).collect()
}

/// The real lookup: the PATH and the usual folders.
pub fn on_path(prog: &str) -> Option<String> {
    // the PATH, the usual folders, and the folder the CLI catalogue links its downloads into (a downloaded ffmpeg)
    iyakki_agent::player::find_program(prog)
}

/// What the Media page says about the players (the explanation when there is none).
pub fn players_text(found: &[(String, String)]) -> (String, bool) {
    if found.is_empty() {
        ("Install the media player: IYAKKI menu \u{2192} Quick install \u{2192} Media (a static ffmpeg build from its builder, GPLv3, checked with its checksum). Until then MP3 plays in IYAKKI itself; video needs the player (or the web image, whose MEDIA plays video in the page engine).".into(), false)
    } else if found[0].0 == "ffmpeg" {
        (format!("Player: IYAKKI's own, decoding with ffmpeg ({}) \u{b7} plays video and sound, with the output of Settings \u{2192} Sound", found[0].1), true)
    } else {
        (format!("Player: {} ({}){}", found[0].0, found[0].1, if found.len() > 1 { format!(" · also {}", found[1..].iter().map(|f| f.0.as_str()).collect::<Vec<_>>().join(", ")) } else { String::new() }), true)
    }
}

/// One row of the Mac drivers page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacRow {
    pub title: &'static str,
    pub state: String,
    pub level: crate::devpop::Level,
    pub how: &'static str,
}

/// The Mac drivers page from a Devices reading: Wi-Fi, T2 audio (the Sound row and the T2 driver entry), Bluetooth, the camera.
pub fn mac_rows(snap: Option<&crate::devpop::Snapshot>) -> Vec<MacRow> {
    use crate::devpop::Level;
    let item = |title: &str| snap.and_then(|s| s.items.iter().find(|i| i.title == title).cloned());
    let help = |words: &[&str]| snap.and_then(|s| s.driver_help.iter().find(|(n, _)| words.iter().any(|w| n.to_lowercase().contains(w))).map(|(n, w)| format!("{n}: {w}")));
    let row = |title: &'static str, it: Option<crate::devpop::Item>, extra: Option<String>, how: &'static str| -> MacRow {
        let (mut state, level) = match it {
            Some(i) => (i.detail, i.level),
            None if snap.is_none() => ("looking...".to_string(), Level::Absent),
            None => ("not found".to_string(), Level::Missing),
        };
        if let Some(e) = extra {
            state = format!("{state} · {e}");
        }
        MacRow { title, state, level, how }
    };
    vec![
        row("Wi-Fi firmware", item("Wi-Fi"), help(&["wi-fi", "wifi"]), "On the Mac's own macOS run dist/drivers/mac-wifi-firmware-from-macos.sh: it copies the Broadcom firmware to a USB stick. Then STORE \u{2192} Drivers \u{2192} the Wi-Fi entry \u{2192} Install from folder or USB."),
        row("T2 audio", item("Sound"), help(&["t2", "audio"]), "T2 Macs (2018 and later) play through the apple-bce driver of the Mac image; this page only reports what the system sees (Settings \u{2192} Sound picks the output)."),
        row("Bluetooth", item("Bluetooth"), help(&["bluetooth"]), "Broadcom Bluetooth needs its .hcd firmware: STORE \u{2192} Drivers \u{2192} Bluetooth firmware \u{2192} Install from folder or USB."),
        row("USB webcam", item("Camera"), help(&["camera", "facetime"]), "A USB (UVC) camera works when plugged in. The built-in FaceTime HD camera needs the Mac image and Apple's firmware file (STORE \u{2192} Drivers \u{2192} Extract firmware)."),
    ]
}

pub struct BrandMenu {
    pub open: bool,
    pub page: Page,
    sel: usize,
    scroll: usize,
    /// Lines of the About page that fit (from the last drawing).
    visible: usize,
    /// The About page's wrapped lines for the width they were made for.
    wrapped: Option<(u32, Vec<(String, crate::about::Style)>)>,
    about: Option<Vec<(String, crate::about::Style)>>,
    /// What is installed (the shell fills it when Quick install opens).
    pub presence: Vec<(Item, Presence)>,
    /// What the vendors publish as newer, for the installed ones (looked up in the background when Quick install opens).
    pub latest: Vec<(Item, Latest)>,
    /// Items whose last install failed (Retry is offered on them).
    pub failed: Vec<Item>,
    /// A desktop window: IYAKKI never installs a cluster there (the dev cluster's rule).
    pub window: bool,
    pub players: Vec<(String, String)>,
    pub path: TextField,
    pub mac: Option<crate::devpop::Snapshot>,
    /// A line under the page: what just happened (good or not).
    pub note: Option<(String, bool)>,
    /// Where the panel hangs from (the name in the top bar), set by the shell.
    pub anchor: (f32, f32),
    /// The person is an administrator (Developer mode is listed only then).
    pub admin: bool,
    /// Node.js is on this machine (npm tools then need nothing first).
    /// Installed by IYAKKI but not runnable, with the plain reason (Reinstall is offered).
    pub unhealthy: Vec<(Item, String)>,
    /// Claude Code / GitHub Copilot: whether each is signed in (the shell fills it from the remembered status; see agent_login.rs).
    pub signin: Vec<(Item, crate::agent_login::State)>,
    /// What the catalogue installs before an item (uv, then Python 3; Node.js), for the order line.
    pub runtimes: Vec<(Item, Vec<&'static str>)>,
    /// A runtime that installed tools use (the sentence to show before replacing it).
    pub used_by: Vec<(Item, String)>,
    /// An item found on the machine (put there by hand): the first press asked whether to install IYAKKI's own copy, the second does.
    confirm_own: Option<Item>,
}

impl Default for BrandMenu {
    fn default() -> BrandMenu {
        BrandMenu::new()
    }
}

impl BrandMenu {
    pub fn new() -> BrandMenu {
        BrandMenu { open: false, page: Page::Menu, sel: 0, scroll: 0, visible: 10, wrapped: None, about: None, presence: Vec::new(), latest: Vec::new(), failed: Vec::new(), window: false, players: Vec::new(), path: TextField::new("/path/to/a/video-or-audio-file", false), mac: None, note: None, anchor: (24.0, 76.0), admin: false, unhealthy: Vec::new(), signin: Vec::new(), runtimes: Vec::new(), used_by: Vec::new(), confirm_own: None }
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.page = Page::Menu;
            self.sel = 0;
            self.note = None;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.page = Page::Menu;
        self.note = None;
    }

    pub fn sel(&self) -> usize {
        self.sel
    }

    /// The entries of the first page for this person (Developer mode only for administrators).
    pub fn shown(&self) -> Vec<(Page, &'static str, &'static str, char)> {
        ENTRIES.iter().copied().filter(|e| e.0 != Page::Developer || self.admin).collect()
    }

    /// The About lines (made once; tests may give their own).
    pub fn set_about(&mut self, lines: Vec<(String, crate::about::Style)>) {
        self.about = Some(lines);
        self.wrapped = None;
    }

    fn about_lines(&mut self) -> &Vec<(String, crate::about::Style)> {
        self.about.get_or_insert_with(crate::about::this_machine)
    }

    pub fn open_page(&mut self, p: Page) -> Outcome {
        if p == Page::Settings {
            return Outcome::Jump("settings", String::new());
        }
        if p == Page::Developer {
            return Outcome::Jump("settings", "tab:developer".into());
        }
        self.page = p;
        self.sel = 0;
        self.scroll = 0;
        self.note = None;
        if p == Page::About {
            self.about_lines();
        }
        if p == Page::Menu { Outcome::Nothing } else { Outcome::Opened(p) }
    }

    fn back(&mut self) -> Outcome {
        if self.page == Page::Menu {
            Outcome::Close
        } else {
            let from = self.shown().iter().position(|e| e.0 == self.page).unwrap_or(0);
            self.open_page(Page::Menu);
            self.sel = from;
            Outcome::Nothing
        }
    }

    fn rows(&self) -> usize {
        match self.page {
            Page::Menu => self.shown().len(),
            Page::Install => Item::ALL.len(),
            _ => 0,
        }
    }

    /// The order the selected Quick install row would run in.
    pub fn install_order(&self) -> Vec<Item> {
        let target = Item::ALL[self.sel.min(Item::ALL.len() - 1)];
        let present = |i: Item| self.presence.iter().any(|(x, p)| *x == i && p.present());
        quickinstall::order(target, &present)
    }

    fn scroll_by(&mut self, d: i32) {
        let total = self.wrapped.as_ref().map_or(0, |w| w.1.len());
        let max = total.saturating_sub(self.visible.max(1));
        self.scroll = (self.scroll as i32 + d).clamp(0, max as i32) as usize;
    }

    /// What Quick install learnt about this machine (read off the screen's thread).
    pub fn set_facts(&mut self, f: quickinstall::Facts) {
        self.presence = f.presence;
        self.unhealthy = f.unhealthy;
        self.runtimes = f.runtimes;
        self.used_by = f.used_by;
        self.confirm_own = None;
    }

    fn install_selected(&mut self) -> Outcome {
        let target = Item::ALL[self.sel.min(Item::ALL.len() - 1)];
        // something found on the machine (put there by hand) is left as it is unless asked twice
        let elsewhere = self.presence.iter().any(|(x, p)| *x == target && matches!(p, Presence::Elsewhere(_))) && target.cli_id().is_some();
        if elsewhere && self.confirm_own != Some(target) {
            self.confirm_own = Some(target);
            self.note = Some((format!("{} is already on this machine and works; IYAKKI leaves it alone. Press Install again to put IYAKKI's own copy next to it (it replaces the links in the tools folder).", target.title()), false));
            return Outcome::Nothing;
        }
        self.confirm_own = None;
        if self.window && (target == Item::K3s || target.needs().contains(&Item::K3s)) {
            self.note = Some(("In a desktop window IYAKKI never installs a cluster: this computer manages its own. Use Quick install on the IYAKKI machine itself.".into(), false));
            return Outcome::Nothing;
        }
        Outcome::Install(Item::ALL[self.sel.min(Item::ALL.len() - 1)])
    }

    pub fn handle_key(&mut self, k: &Key) -> Outcome {
        if matches!(k, Key::F(10)) {
            return Outcome::Close;
        }
        // the Media page's field takes what is typed
        if self.page == Page::Media {
            match k {
                Key::Esc => return self.back(),
                Key::Enter => {
                    let p = self.path.value.trim().to_string();
                    if p.is_empty() {
                        self.note = Some(("Type the path of a file first (for example /usr/share/iyakki/samples/...).".into(), false));
                        return Outcome::Nothing;
                    }
                    return Outcome::Play(p);
                }
                other => {
                    self.path.handle(other);
                    return Outcome::Nothing;
                }
            }
        }
        let n = self.rows();
        match k {
            Key::Esc | Key::Backspace | Key::Left => self.back(),
            Key::Down | Key::Tab if n > 0 => {
                self.sel = (self.sel + 1) % n;
                Outcome::Nothing
            }
            Key::Up | Key::BackTab if n > 0 => {
                self.sel = (self.sel + n - 1) % n;
                Outcome::Nothing
            }
            Key::Down => {
                self.scroll_by(1);
                Outcome::Nothing
            }
            Key::Up => {
                self.scroll_by(-1);
                Outcome::Nothing
            }
            Key::PageDown => {
                self.scroll_by(self.visible.max(2) as i32 - 1);
                Outcome::Nothing
            }
            Key::PageUp => {
                self.scroll_by(-(self.visible.max(2) as i32 - 1));
                Outcome::Nothing
            }
            Key::Home => {
                self.sel = 0;
                self.scroll = 0;
                Outcome::Nothing
            }
            Key::End => {
                self.sel = n.saturating_sub(1);
                self.scroll_by(i32::MAX / 2);
                Outcome::Nothing
            }
            Key::Enter | Key::Right => match self.page {
                Page::Menu => {
                    let e = self.shown();
                    self.open_page(e[self.sel.min(e.len() - 1)].0)
                }
                Page::Install => self.install_selected(),
                Page::Mac => Outcome::Jump("store", "drivers".into()),
                _ => Outcome::Nothing,
            },
            Key::Char(c) => {
                let c = c.to_ascii_lowercase();
                match self.page {
                    Page::Menu => match self.shown().iter().position(|e| e.3 == c) {
                        Some(i) => {
                            self.sel = i;
                            self.open_page(self.shown()[i].0)
                        }
                        None => Outcome::Nothing,
                    },
                    Page::Install if c == 'i' => self.install_selected(),
                    Page::Install if c == 'r' => Outcome::ShowInstalls,
                    Page::Mac if c == 'd' => Outcome::Jump("store", "drivers".into()),
                    Page::Mac if c == 'v' => Outcome::Jump("devices", String::new()),
                    _ => Outcome::Nothing,
                }
            }
            _ => Outcome::Nothing,
        }
    }

    /// A click on one of the panel's own regions.
    pub fn handle_hit(&mut self, ev: &Event) -> Outcome {
        match ev {
            Event::Tab(CLOSE) => Outcome::Close,
            Event::Tab(BACK) => self.back(),
            Event::Tab(n) if (ENTRY..ENTRY + self.shown().len()).contains(n) => {
                self.sel = n - ENTRY;
                self.open_page(self.shown()[self.sel].0)
            }
            Event::Tab(n) if (IROW..IROW + Item::ALL.len()).contains(n) => {
                self.sel = n - IROW;
                self.note = None;
                Outcome::Nothing
            }
            Event::Tab(INSTALL_BTN) => self.install_selected(),
            Event::Tab(RUNS_BTN) => Outcome::ShowInstalls,
            Event::Tab(PLAY) => self.handle_key(&Key::Enter),
            Event::Tab(STOP) => Outcome::StopMedia,
            Event::Tab(OPEN_MEDIA) => Outcome::Jump("media", String::new()),
            Event::Tab(INSTALL_PLAYER) => Outcome::Install(Item::Ffmpeg),
            Event::Tab(DRIVERS) => Outcome::Jump("store", "drivers".into()),
            Event::Tab(DEVICES) => Outcome::Jump("devices", String::new()),
            Event::FocusAt(PATH_FIELD, x) => {
                self.path.click(*x);
                Outcome::Nothing
            }
            Event::Paste(t) if self.page == Page::Media => {
                self.path.paste(t);
                Outcome::Nothing
            }
            Event::Scroll(d) => {
                if self.page == Page::About {
                    self.scroll_by(-*d * 3);
                }
                Outcome::Nothing
            }
            _ => Outcome::Nothing,
        }
    }

    // ------------------------------------------------------------------------------------------------ drawing

    /// The panel's rectangle: under the anchor, kept on screen; the first page is compact, the others taller.
    pub fn panel(&self, w: u32, h: u32, s: f32) -> R {
        let (ax, ay) = self.anchor;
        let pw = (if self.page == Page::Menu { 470.0 } else { 600.0 } * s).min(w as f32 - 24.0 * s);
        let want = match self.page {
            Page::Menu => (66.0 + 62.0 * self.shown().len() as f32 + 34.0) * s,
            Page::Install => 660.0 * s,
            Page::Media => 380.0 * s,
            _ => 640.0 * s,
        };
        let ph = want.min(h as f32 - ay - 12.0 * s).max(160.0 * s);
        let x = ax.clamp(12.0 * s, (w as f32 - pw - 12.0 * s).max(12.0 * s));
        R::new(x, ay, pw, ph)
    }

    /// Draw it. Registers its own click regions and clears the others (a click outside closes it).
    pub fn render(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32, tick: u64) {
        if !self.open {
            return;
        }
        g.clear_hits();
        let p = self.panel(w, h, s);
        crate::fx::modal_panel(g, p, s, ACCENT, false);
        // the header: the name (with its colour strip) or Back and the page's title, and Close
        let title = ENTRIES.iter().find(|e| e.0 == self.page).map_or("IYAKKI", |e| e.1);
        let mut tx = p.x + 20.0 * s;
        if self.page != Page::Menu {
            let b = R::new(p.x + 10.0 * s, p.y + 10.0 * s, 34.0 * s, 32.0 * s);
            let hot = g.hovered(b);
            crate::fx::chrome_button(g, b, s, hot, ACCENT);
            let aw = g.text_width(17.0 * s, "\u{2190}", Face::Bold);
            g.text(b.x + (b.w - aw) / 2.0, b.y + 5.0 * s, 17.0 * s, MUTED, "\u{2190}", Face::Bold);
            g.hit(b, Event::Tab(BACK), "back");
            tx = b.x + b.w + 10.0 * s;
        }
        g.text(tx, p.y + 13.0 * s, 19.0 * s, TEXT, title, Face::Bold);
        if self.page == Page::Menu {
            let nw = g.text_width(19.0 * s, title, Face::Bold);
            let v = format!("{} \u{00b7} an OS for agents", iyakki_agent::relinfo::VERSION);
            let v = g.fit(12.0 * s, &v, Face::Sans, p.w - nw - 110.0 * s);
            g.text(tx + nw + 12.0 * s, p.y + 19.0 * s, 12.0 * s, DIM, &v, Face::Sans);
        }
        let close = R::new(p.x + p.w - 44.0 * s, p.y + 8.0 * s, 34.0 * s, 34.0 * s);
        let hot = g.hovered(close);
        crate::fx::chrome_button(g, close, s, hot, ACCENT);
        let cw = g.text_width(18.0 * s, "x", Face::Bold);
        g.text(close.x + (close.w - cw) / 2.0, close.y + 6.0 * s, 18.0 * s, MUTED, "x", Face::Bold);
        g.hit(close, Event::Tab(CLOSE), "close");
        let body = R::new(p.x + 16.0 * s, p.y + 54.0 * s, p.w - 32.0 * s, p.h - 54.0 * s - 34.0 * s);
        match self.page {
            Page::Menu => self.draw_menu(g, body, s),
            Page::About => self.draw_about(g, body, s),
            Page::Install => self.draw_install(g, body, s),
            Page::Media => self.draw_media(g, body, s, tick),
            Page::Mac => self.draw_mac(g, body, s),
            Page::Settings | Page::Developer => {}
        }
        // the foot: what just happened, else the keys
        let (foot, col) = match &self.note {
            Some((t, ok)) => (t.clone(), if *ok { OK } else { AMBER }),
            None => (
                match self.page {
                    Page::Menu => "Up/Down and Enter, or the letter · Esc or F10 closes",
                    Page::About => "Up/Down, PageUp/PageDown or the wheel scroll · Esc back",
                    Page::Install => "Up/Down choose · Enter installs · r installer runs · Esc back",
                    Page::Media => "Type a path · Enter plays · Esc back",
                    Page::Mac => "Enter or d: STORE \u{2192} Drivers · v: Devices · Esc back",
                    Page::Settings | Page::Developer => "",
                }
                .to_string(),
                DIM,
            ),
        };
        let foot = g.fit(12.0 * s, &foot, Face::Sans, p.w - 36.0 * s);
        g.text(p.x + 18.0 * s, p.y + p.h - 26.0 * s, 12.0 * s, col, &foot, Face::Sans);
    }

    fn draw_menu(&mut self, g: &mut Gfx, body: R, s: f32) {
        let rh = 62.0 * s;
        for (i, (page, title, about, key)) in self.shown().into_iter().enumerate() {
            let r = R::new(body.x - 6.0 * s, body.y + i as f32 * rh, body.w + 12.0 * s, rh - 6.0 * s);
            let hot = g.hovered(r);
            if hot {
                self.sel = i;
            }
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.sel, hot, ACCENT);
            let icon = match page {
                Page::About => Icon::System,
                Page::Install => Icon::Cli,
                Page::Media => Icon::Media,
                Page::Mac => Icon::Devices,
                Page::Developer => Icon::Ide,
                _ => Icon::Settings,
            };
            let d = 36.0 * s;
            g.rrect(r.x + 10.0 * s, r.y + (r.h - d) / 2.0, d, d, 10.0 * s, rgb(28, 40, 70));
            g.icon(icon, r.x + 10.0 * s + d / 2.0, r.y + r.h / 2.0, d * 0.62, if i == self.sel { ACCENT } else { MUTED });
            let tx = r.x + 10.0 * s + d + 12.0 * s;
            g.text(tx, r.y + 7.0 * s, 15.0 * s, TEXT, title, Face::Bold);
            let kw = g.text_width(12.0 * s, &key.to_string(), Face::Mono);
            let ab = g.fit(12.5 * s, about, Face::Sans, r.x + r.w - tx - kw - 30.0 * s);
            g.text(tx, r.y + 7.0 * s + g.line_height(15.0 * s, Face::Bold) + 1.0 * s, 12.5 * s, MUTED, &ab, Face::Sans);
            // the key, in a small box at the right
            let kb = R::new(r.x + r.w - kw - 26.0 * s, r.y + (r.h - 22.0 * s) / 2.0, kw + 14.0 * s, 22.0 * s);
            g.rrect_outline(kb.x, kb.y, kb.w, kb.h, 6.0 * s, 1.0, crate::fx::with_alpha(ACCENT, 120));
            g.text(kb.x + 7.0 * s, kb.y + 3.0 * s, 12.0 * s, DIM, &key.to_string(), Face::Mono);
            g.hit(r, Event::Tab(ENTRY + i), title);
        }
    }

    fn draw_about(&mut self, g: &mut Gfx, body: R, s: f32) {
        use crate::about::Style;
        let lh = 19.0 * s;
        let width_key = (body.w as u32) ^ ((s * 100.0) as u32) << 20;
        if self.wrapped.as_ref().is_none_or(|w| w.0 != width_key) {
            let src = self.about_lines().clone();
            let mut out: Vec<(String, Style)> = Vec::new();
            let label_w = 92.0 * s;
            for (t, st) in src {
                let (size, face, avail) = match st {
                    Style::Heading => (15.0 * s, Face::Bold, body.w - 16.0 * s),
                    Style::Licence => (13.5 * s, Face::Bold, body.w - 16.0 * s),
                    Style::Fact => (13.0 * s, Face::Sans, body.w - label_w - 16.0 * s),
                    _ => (12.5 * s, Face::Sans, body.w - 30.0 * s),
                };
                if t.is_empty() || st == Style::Fact {
                    out.push((t, st));
                    continue;
                }
                for l in g.wrap(size, &t, face, avail) {
                    out.push((l, st));
                }
            }
            self.wrapped = Some((width_key, out));
        }
        let lines = self.wrapped.as_ref().map(|w| w.1.clone()).unwrap_or_default();
        self.visible = ((body.h / lh).floor() as usize).max(1);
        let max = lines.len().saturating_sub(self.visible);
        self.scroll = self.scroll.min(max);
        g.hit(body, Event::Tab(SCROLL), "scroll");
        for (k, (t, st)) in lines.iter().enumerate().skip(self.scroll).take(self.visible) {
            let y = body.y + (k - self.scroll) as f32 * lh;
            match st {
                Style::Heading => {
                    g.text(body.x, y, 15.0 * s, ACCENT, t, Face::Bold);
                }
                Style::Licence => {
                    g.text(body.x + 4.0 * s, y + 1.0 * s, 13.5 * s, TEXT, t, Face::Bold);
                }
                Style::Fact => {
                    let (l, v) = t.split_once('\t').unwrap_or((t.as_str(), ""));
                    g.text(body.x + 4.0 * s, y + 1.0 * s, 13.0 * s, MUTED, l, Face::Sans);
                    let v = g.fit(13.0 * s, v, Face::Bold, body.w - 110.0 * s);
                    g.text(body.x + 96.0 * s, y + 1.0 * s, 13.0 * s, TEXT, &v, Face::Bold);
                }
                Style::Item => {
                    g.text(body.x + 18.0 * s, y + 2.0 * s, 12.5 * s, MUTED, t, Face::Sans);
                }
                Style::Note => {
                    g.text(body.x + 4.0 * s, y + 2.0 * s, 12.5 * s, DIM, t, Face::Sans);
                }
            }
        }
        crate::fx::scrollbar(g, R::new(body.x + body.w + 6.0 * s, body.y, 4.0 * s, body.h), s, self.scroll, self.visible, lines.len(), ACCENT);
    }

    fn draw_install(&mut self, g: &mut Gfx, body: R, s: f32) {
        // the list: a title per section, then its items; the detail of the chosen one below
        let detail_h = 172.0 * s;
        let list = R::new(body.x, body.y, body.w, (body.h - detail_h).max(80.0 * s));
        let (hh, rh) = (26.0 * s, 46.0 * s);
        let mut lines: Vec<(Option<quickinstall::Group>, Option<usize>)> = Vec::new();
        for gr in quickinstall::Group::ALL {
            lines.push((Some(gr), None));
            for it in quickinstall::items_of(gr) {
                lines.push((None, Item::ALL.iter().position(|x| *x == it)));
            }
        }
        let height = |l: &(Option<quickinstall::Group>, Option<usize>)| if l.0.is_some() { hh } else { rh };
        // keep the chosen row in view
        let sel_line = lines.iter().position(|l| l.1 == Some(self.sel)).unwrap_or(0);
        let mut first = self.scroll.min(sel_line.saturating_sub(1));
        loop {
            let used: f32 = lines[first..=sel_line].iter().map(height).sum();
            if used <= list.h || first >= sel_line {
                break;
            }
            first += 1;
        }
        self.scroll = first;
        let mut y = list.y;
        for l in lines.iter().skip(first) {
            if y + height(l) > list.y + list.h + 0.5 {
                break;
            }
            match l {
                (Some(gr), _) => {
                    g.text(list.x, y + 5.0 * s, 13.0 * s, ACCENT, gr.title(), Face::Bold);
                    let tw = g.text_width(13.0 * s, gr.title(), Face::Bold);
                    let note = g.fit(11.5 * s, gr.note(), Face::Sans, list.w - tw - 12.0 * s);
                    g.text(list.x + tw + 10.0 * s, y + 7.0 * s, 11.5 * s, DIM, &note, Face::Sans);
                    y += hh;
                }
                (None, Some(i)) => {
                    let (i, it) = (*i, Item::ALL[*i]);
                    let r = R::new(list.x - 6.0 * s, y, list.w + 12.0 * s, rh - 4.0 * s);
                    let hot = g.hovered(r);
                    crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel, hot, ACCENT);
                    let pres = self.presence.iter().find(|(x, _)| *x == it).map(|(_, p)| p.clone());
                    let dot = match &pres {
                        Some(Presence::Installed(_)) => OK,
                        Some(Presence::Elsewhere(_)) => AMBER,
                        _ => DIM,
                    };
                    g.circle(r.x + 14.0 * s, r.y + r.h / 2.0, 4.5 * s, dot);
                    let tx = r.x + 28.0 * s;
                    g.text(tx, r.y + 4.0 * s, 14.0 * s, TEXT, it.title(), Face::Bold);
                    let lat = self.latest.iter().find(|(x, _)| *x == it).map(|(_, l)| l);
                    let mut state = pres.clone().map_or("checking...".to_string(), |p| quickinstall::state_text(&p, lat));
                    if self.failed.contains(&it) {
                        state = format!("last install failed · {state}");
                    } else if self.unhealthy.iter().any(|(x, _)| *x == it) {
                        state = format!("installed but not runnable · {state}");
                    }
                    // an installed coding agent says who is signed in, or that nobody is
                    if let (Some(Presence::Installed(_)), Some((_, st))) = (&pres, self.signin.iter().find(|(x, _)| *x == it)) {
                        match st {
                            crate::agent_login::State::SignedIn(who) => state = format!("signed in as {who} · {state}"),
                            crate::agent_login::State::SignedOut => state = format!("not signed in · {state}"),
                            _ => {}
                        }
                    }
                    let state = g.fit(11.5 * s, &state, Face::Sans, r.w * 0.42);
                    let sw = g.text_width(11.5 * s, &state, Face::Sans);
                    g.text(r.x + r.w - sw - 12.0 * s, r.y + 6.0 * s, 11.5 * s, dot, &state, Face::Sans);
                    let ab = g.fit(11.5 * s, it.about(), Face::Sans, r.w - 40.0 * s);
                    g.text(tx, r.y + 6.0 * s + g.line_height(14.0 * s, Face::Bold), 11.5 * s, MUTED, &ab, Face::Sans);
                    g.hit(r, Event::Tab(IROW + i), it.title());
                    y += rh;
                }
                _ => {}
            }
        }
        // the detail: order, licence, buttons
        let target = Item::ALL[self.sel.min(Item::ALL.len() - 1)];
        let mut y = list.y + list.h + 6.0 * s;
        let order = self.install_order();
        let runtimes = self.runtimes.clone();
        let titles = quickinstall::order_titles_with(&order, &|i: Item| runtimes.iter().find(|(x, _)| *x == i).map(|(_, v)| v.clone()).unwrap_or_default());
        let t = format!("Order: {}", quickinstall::titles_text(&titles));
        let t = g.fit(13.5 * s, &t, Face::Bold, body.w);
        g.text(body.x, y, 13.5 * s, TEXT, &t, Face::Bold);
        y += 20.0 * s;
        let lic = format!("Licence: {}", target.licence());
        let unclear = target.licence().contains("open owner decision");
        for l in g.wrap(12.0 * s, &lic, Face::Sans, body.w).iter().take(3) {
            g.text(body.x, y, 12.0 * s, if unclear { AMBER } else { MUTED }, l, Face::Sans);
            y += 16.0 * s;
        }
        // what to know before pressing: not runnable, found on the machine, used by other tools
        let bad = self.unhealthy.iter().find(|(x, _)| *x == target).map(|(_, m)| m.clone());
        let mut warn: Vec<(String, [u8; 4])> = Vec::new();
        if let Some(m) = &bad {
            warn.push((format!("Installed but not runnable: {m}"), ERR));
        }
        if let Some((_, Presence::Elsewhere(d))) = self.presence.iter().find(|(x, _)| *x == target) {
            if target.cli_id().is_some() {
                warn.push((format!("Found on this machine, left as it is: {d}"), MUTED));
            }
        }
        if let Some((_, w)) = self.used_by.iter().find(|(x, _)| *x == target) {
            warn.push((w.clone(), AMBER));
        }
        for (t, c) in warn.iter().take(2) {
            if let Some(l) = g.wrap(12.0 * s, t, Face::Sans, body.w).first() {
                g.text(body.x, y, 12.0 * s, *c, l, Face::Sans);
                y += 16.0 * s;
            }
        }
        y += 6.0 * s;
        let pres = self.presence.iter().find(|(x, _)| *x == target).map(|(_, p)| p.clone()).unwrap_or(Presence::Missing);
        let lat = self.latest.iter().find(|(x, _)| *x == target).map(|(_, l)| l);
        let label = if target == Item::IndicTts {
            "Deploy Indic-TTS (review)".to_string()
        } else if self.failed.contains(&target) {
            format!("Retry {}", target.title())
        } else if bad.is_some() {
            format!("Reinstall {} (repair)", target.title())
        } else {
            quickinstall::action_label(target, &pres, lat)
        };
        let bw = if self.window && target.needs().contains(&Item::K3s) || self.window && target == Item::K3s { g.button_off(body.x, y, 14.0 * s, &label) } else { g.button(body.x, y, 14.0 * s, &label, ACCENT, Event::Tab(INSTALL_BTN)) };
        let rw = g.button(body.x + bw + 10.0 * s, y, 14.0 * s, "Installer runs", MUTED, Event::Tab(RUNS_BTN));
        // Claude Code and Copilot, once installed: Login (and Logout) with the account
        if let (Some(Presence::Installed(_)), Some(agent)) = (self.presence.iter().find(|(x, _)| *x == target).map(|(_, p)| p), target.cli_id().and_then(crate::agent_login::agent_of_cli)) {
            let st = self.signin.iter().find(|(x, _)| *x == target).map_or(crate::agent_login::State::Unknown("checking…".into()), |(_, s)| s.clone());
            // on the row of the install button when there is room, else on the row below
            let (bx, by) = if body.x + bw + rw + 30.0 * s + 330.0 * s < body.x + body.w { (body.x + bw + rw + 30.0 * s, y) } else { (body.x, y + 36.0 * s) };
            let used = crate::agent_login::buttons(g, bx, by, 14.0 * s, agent, &st);
            let (t, col) = crate::agent_login::chip(&st);
            let t = g.fit(12.0 * s, &t, Face::Sans, (body.x + body.w - bx - used).max(40.0 * s));
            g.text(bx + used, by + 8.0 * s, 12.0 * s, col, &t, Face::Sans);
        }
    }

    fn draw_media(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let mut y = body.y;
        let (txt, ok) = players_text(&self.players);
        for l in g.wrap(13.0 * s, &txt, Face::Sans, body.w).iter().take(4) {
            g.text(body.x, y, 13.0 * s, if ok { OK } else { AMBER }, l, Face::Sans);
            y += 18.0 * s;
        }
        if !self.players.iter().any(|p| p.0 == "ffmpeg") {
            g.button(body.x, y, 14.0 * s, "Install the media player", ACCENT, Event::Tab(INSTALL_PLAYER));
            y += 40.0 * s;
        }
        y += 10.0 * s;
        g.text(body.x, y, 13.0 * s, MUTED, "File to play", Face::Sans);
        y += 20.0 * s;
        let fr = R::new(body.x, y, body.w, 40.0 * s);
        self.path.render(g, fr, true, s, (tick / 15) % 2 == 0, Face::Sans, PATH_FIELD);
        y += 52.0 * s;
        let mut x = body.x;
        x += g.button(x, y, 14.0 * s, "Play", ACCENT, Event::Tab(PLAY)) + 10.0 * s;
        x += g.button(x, y, 14.0 * s, "Stop", ERR, Event::Tab(STOP)) + 10.0 * s;
        g.button(x, y, 14.0 * s, "Open MEDIA", MUTED, Event::Tab(OPEN_MEDIA));
        y += 44.0 * s;
        if let Some(now) = iyakki_agent::media::shared().now_playing() {
            let t = g.fit(13.0 * s, &format!("\u{25b6} {now}"), Face::Sans, body.w);
            g.text(body.x, y, 13.0 * s, OK, &t, Face::Sans);
            y += 20.0 * s;
        }
        for l in g.wrap(12.0 * s, "Video and sound play in MEDIA's own player (ffmpeg decodes; Space pauses, arrows seek, f is fullscreen), with the output of Settings \u{2192} Sound; MP3 plays inside IYAKKI. MEDIA has the library, the queue and the sample files.", Face::Sans, body.w).iter().take(3) {
            g.text(body.x, y, 12.0 * s, DIM, l, Face::Sans);
            y += 16.0 * s;
        }
    }

    fn draw_mac(&mut self, g: &mut Gfx, body: R, s: f32) {
        use crate::devpop::Level;
        let rows = mac_rows(self.mac.as_ref());
        let mut y = body.y;
        let bottom = body.y + body.h - 44.0 * s;
        for r in &rows {
            if y > bottom - 40.0 * s {
                break;
            }
            let col = match r.level {
                Level::Ok => OK,
                Level::Warn => AMBER,
                Level::Missing => ERR,
                Level::Absent => DIM,
            };
            g.circle(body.x + 6.0 * s, y + 9.0 * s, 5.0 * s, col);
            g.text(body.x + 20.0 * s, y, 15.0 * s, TEXT, r.title, Face::Bold);
            y += 21.0 * s;
            let st = g.fit(12.5 * s, &r.state, Face::Sans, body.w - 20.0 * s);
            g.text(body.x + 20.0 * s, y, 12.5 * s, col, &st, Face::Sans);
            y += 18.0 * s;
            for l in g.wrap(12.0 * s, r.how, Face::Sans, body.w - 20.0 * s).iter().take(3) {
                g.text(body.x + 20.0 * s, y, 12.0 * s, MUTED, l, Face::Sans);
                y += 16.0 * s;
            }
            y += 10.0 * s;
        }
        let by = (y + 4.0 * s).min(bottom);
        let bw = g.button(body.x, by, 14.0 * s, "STORE \u{2192} Drivers", ACCENT, Event::Tab(DRIVERS));
        g.button(body.x + bw + 10.0 * s, by, 14.0 * s, "Devices", MUTED, Event::Tab(DEVICES));
    }
}

#[cfg(test)]
#[path = "brandmenu_tests.rs"]
mod tests;
