//! The IYAKKI menu: a click on the "IYAKKI" name in the launcher's top bar (or F10 anywhere after sign-in) opens a dropdown in
//! the design system's glass look with four entries, each a page of the same panel:
//!
//! * **About IYAKKI**: version, build, variant, kernel, IYAKKI's licence (Apache-2.0) and a scrollable list of the licences of
//!   everything inside (the Rust crates and the programs of the system images; [`crate::about`]).
//! * **Quick install** (key q): no page of its own any more; it opens the Store on its Setup tab (`apps/store_setup.rs`), where the
//!   installs are cards. The jobs are still [`iyakki_agent::quickinstall`]'s.
//! * **Media player**: which player this image has (ffplay, mpv, VLC, mplayer; MP3 plays in IYAKKI itself), a file to play, Stop,
//!   and MEDIA for the library.
//! * **Mac drivers**: Wi-Fi firmware (the script for macOS, then "Install from folder or USB"), T2 audio, Bluetooth and the USB
//!   camera, as the Devices popover reads them, and STORE → Drivers.
//!
//! Keys: Up/Down and Enter, the underlined letters, Esc goes back a page and closes on the first page, F10 closes. A click
//! outside the panel closes it. The menu only says what it wants ([`Outcome`]); the shell does it (jobs, the player, jumps).

use crate::safeclamp::SafeClamp;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_agent::quickinstall::Item;

#[path = "brandmenu_gpu.rs"]
mod gpu_page;
#[path = "brandmenu_linux.rs"]
mod linux_page;
pub use linux_page::{BoxRow, ImageRow, LView, LinuxAction, LinuxFacts, LinuxPage};
/// The ids of the Linux apps page's controls (tests).
pub mod linux_page_ids {
    pub use super::linux_page::{L_CREATE as CREATE, L_TERMINAL as TERMINAL};
}
pub use gpu_page::{GPU_INSTALL, GPU_PAGE, GPU_REFRESH, GPU_TEST};

pub const BASE: usize = 1_200_000;
pub const CLOSE: usize = BASE;
pub const BACK: usize = BASE + 1;
/// A menu entry: `ENTRY + index`.
pub const ENTRY: usize = BASE + 10;
pub const PLAY: usize = BASE + 40;
pub const STOP: usize = BASE + 41;
pub const OPEN_MEDIA: usize = BASE + 42;
pub const PATH_FIELD: usize = BASE + 43;
/// Starts the Quick install job of the media player (ffmpeg) from the Media page.
pub const INSTALL_PLAYER: usize = BASE + 44;
pub const DRIVERS: usize = BASE + 50;
pub const DEVICES: usize = BASE + 51;
/// Mac drivers page: Download from Apple, from the folder or stick in the field, from the disk image in the field, copy the prompt, the field.
pub const MAC_DOWNLOAD: usize = BASE + 70;
pub const MAC_FOLDER: usize = BASE + 71;
pub const MAC_IMAGE: usize = BASE + 72;
pub const MAC_PROMPT: usize = BASE + 73;
pub const MAC_PATH: usize = BASE + 74;
/// Mac drivers page: delete the kept download and unpacked folder.
pub const MAC_CLEAN: usize = BASE + 75;
/// Mac drivers page: "Load the Bluetooth driver anyway".
pub const MAC_BTLOAD: usize = BASE + 76;
pub const SCROLL: usize = BASE + 60;

const ACCENT: [u8; 4] = crate::fx::CYAN;
const AMBER: [u8; 4] = rgb(255, 196, 94);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Menu,
    About,
    /// Not a page: the entry opens the Store on its Setup tab (the long Quick install page moved there).
    Setup,
    Media,
    Mac,
    /// Linux apps: Debian and Ubuntu sandboxes and image apps (`brandmenu_linux.rs`, `docs/LINUX-APPS.md`).
    Linux,
    /// GPU for local models: the GPUs found, their drivers, the pods that use them, the test prompt (`brandmenu_gpu.rs`). Not an entry of the
    /// first page: the GPU cards of Store -> Setup open it ("Open its page").
    Gpu,
    /// Not a page: the entry opens Settings.
    Settings,
    /// Not a page: Settings → Developer mode (administrators only).
    Developer,
    /// Not a page: opens the Cloud dialog (the tenants signed in to, switching, adding one; `shell_tenant.rs`).
    Cloud,
}

/// The entries of the first page: page, title, what it is, the key that opens it.
pub const ENTRIES: [(Page, &str, &str, char); 8] = [
    (Page::About, "About IYAKKI", "Version, build, kernel, and the licences of everything inside", 'a'),
    (Page::Setup, "Quick install", "Opens Store \u{2192} Setup: cluster, AI servers, voices, coding agents, drivers", 'q'),
    (Page::Media, "Media player", "Play a video or audio file", 'm'),
    (Page::Linux, "Linux apps", "Debian and Ubuntu programs in a sandbox, and ready-made image apps", 'l'),
    (Page::Mac, "Mac drivers", "Wi-Fi firmware, T2 audio, Bluetooth and the USB camera", 'd'),
    (Page::Settings, "Settings", "Network, sound, display, time, models, accounts and more", 's'),
    (Page::Developer, "Developer mode", "Let a paired computer install, run and watch apps here (administrators)", 'v'),
    (Page::Cloud, "Cloud", "Tenants you are signed in to: switch between them, add one", 'c'),
];

/// The name the Cloud mode table (`iyakki_agent::cloudmode::MENU_ENTRIES`) knows a page under ("" for the ones that need no hardware).
fn cloud_id(p: Page) -> &'static str {
    match p {
        Page::Media => "media",
        Page::Mac => "mac",
        Page::Menu | Page::About | Page::Setup | Page::Gpu | Page::Linux | Page::Settings | Page::Developer | Page::Cloud => "",
    }
}

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
    /// Run the GPU page's test prompt (a detached job).
    GpuTest,
    /// Open an app at an item (`("store", "drivers")`, `("media", "")`).
    Jump(&'static str, String),
    /// Install Apple's Wi-Fi and Bluetooth firmware from this source (Mac drivers page): a detached job.
    MacFirmware(iyakki_agent::macfw::Source),
    /// Delete the downloaded recovery image and the unpacked folder the firmware job kept.
    MacClean,
    /// "Load the Bluetooth driver anyway": give the held drivers back and load them without firmware (the shell does it).
    MacBtLoad,
    /// A button of the Linux apps page (the shell starts the job, opens the terminal, asks the cluster).
    Linux(LinuxAction),
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
        ("Install the media player: Store \u{2192} Setup \u{2192} Media tools (a static ffmpeg build from its builder, GPLv3, checked with its checksum). Until then MP3 plays in IYAKKI itself; video needs the player (or the web image, whose MEDIA plays video in the page engine).".into(), false)
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
    mac_rows_for(snap, true)
}

/// The same for a Mac with (`t2`) or without a T2 chip: the explanations differ (a Mac without one already has Wi-Fi, with limits).
pub fn mac_rows_for(snap: Option<&crate::devpop::Snapshot>, t2: bool) -> Vec<MacRow> {
    use crate::devpop::Level;
    let item = |title: &str| snap.and_then(|s| s.items.iter().find(|i| i.title == title).cloned());
    let help = |words: &[&str]| snap.and_then(|s| s.driver_help.iter().find(|(n, _)| words.iter().any(|w| n.to_lowercase().contains(w))).map(|(n, w)| format!("{n}: {w}")));
    let row = |title: &'static str, it: Option<crate::devpop::Item>, extra: Option<String>, how: &'static str| -> MacRow {
        let (mut state, level) = match it {
            Some(i) => (i.detail, i.level),
            None if snap.is_none() => ("looking...".to_string(), Level::Absent),
            None => ("not found".to_string(), Level::Missing),
        };
        if let Some(e) = extra.filter(|e| !state.contains(e.as_str())) {
            state = format!("{state} · {e}");
        }
        MacRow { title, state, level, how }
    };
    vec![
        row("Wi-Fi firmware", item("Wi-Fi"), help(&["wi-fi", "wifi"]), if t2 { "Apple's firmware is not shipped. Install it below: from Apple's download for this Mac, or from a folder, a stick or a disk image you have (or run dist/drivers/mac-wifi-firmware-from-macos.sh in macOS first)." } else { "This Mac has no T2 chip: Wi-Fi works without Apple's files (the generic firmware is on the image), with limits such as fewer channels. Download fetches only this Mac's Wi-Fi NVRAM and CLM files, from Apple, when you press it." }),
        row("T2 audio", item("Sound"), help(&["t2", "audio"]), "T2 Macs (2018 and later) play through the apple-bce driver of the Mac image; this page only reports what the system sees (Settings \u{2192} Sound picks the output)."),
        row("Bluetooth", item("Bluetooth"), help(&["bluetooth"]), if t2 { "The same install brings the Bluetooth patch file when macOS has one. The Mac mini 2018's chip (BCM4364) has none in macOS: its controller runs in its ROM state (limited); a USB Bluetooth adapter is the reliable option." } else { "The same download also brings the Bluetooth patch file for the UART chip when macOS has one; without it the controller runs in its ROM state (limited); a USB Bluetooth adapter is the reliable option." }),
        row("USB webcam", item("Camera"), help(&["camera", "facetime"]), "A USB (UVC) camera works when plugged in. The built-in FaceTime HD camera of a MacBook needs the Mac image and Apple's camera firmware: STORE \u{2192} Setup \u{2192} Hardware \u{2192} MacBook FaceTime camera: Install camera driver and firmware (the driver is in the image; about 17.6 MB, from Apple, on your click)."),
    ]
}

/// The notice at the foot of the Mac drivers page: whose files these are, and what the download would ask Apple for on this model.
pub fn mac_notice(model: &str, board_name: &str) -> String {
    let r = iyakki_drivers::macboard::resolve(model, board_name);
    let t2 = if r.t2 || model.is_empty() { "" } else { " This Mac has no T2 chip." };
    let m = match (r.table, &r.board_id) {
        (Some(b), Some(id)) => format!(" This Mac: {} ({}), board id {id}{}.{t2}", b.model, b.name, if r.verified { "" } else { ", not yet tried on a real one" }),
        (None, Some(id)) => format!(" This Mac: {model} is not on IYAKKI's verified list; Download asks Apple for this Mac's own board id {id} and Apple's service decides.{t2}"),
        (_, None) if model.is_empty() => String::new(),
        (_, None) => format!(" This Mac: {model} is not in IYAKKI's table and its board id could not be read, so Download is not offered; use a folder, a stick or a disk image.{t2}"),
    };
    format!("{}{m}", iyakki_agent::macfw::NOTICE)
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
    pub players: Vec<(String, String)>,
    pub path: TextField,
    pub mac: Option<crate::devpop::Snapshot>,
    /// The Mac drivers page: the path typed for "from a folder or stick" / "from a disk image" (click the field to type).
    pub mac_path: TextField,
    mac_focus: bool,
    /// What the last firmware run said (read from the data folder when the page opens and while it is open).
    pub mac_last: Option<iyakki_agent::macfw::Outcome>,
    /// A firmware job is running (the shell fills it).
    pub mac_busy: bool,
    /// Bytes the firmware job keeps on the data volume (a downloaded image after a failure, the unpacked folder).
    pub mac_leftover: u64,
    /// What the firmware job keeps and what Retry does (`macfw::card_status`); empty when nothing is kept.
    pub mac_cache: String,
    /// The Bluetooth drivers are set aside until firmware exists (the shell fills it): "Load the Bluetooth driver anyway" is offered.
    pub mac_bt_held: bool,
    /// The DMI product name, for the sentence about the download ("Macmini8,1: board id ...").
    pub mac_model: String,
    /// The DMI board name (`Mac-B4831CEBD52A0C4C`): the board id Apple's service is asked for.
    pub mac_board: String,
    /// A line under the page: what just happened (good or not).
    pub note: Option<(String, bool)>,
    /// Where the panel hangs from (the name in the top bar), set by the shell.
    pub anchor: (f32, f32),
    /// The person is an administrator (Developer mode is listed only then).
    pub admin: bool,
    /// The GPU page's facts (the shell fills them when the page opens).
    pub gpu: Option<iyakki_agent::gpu::Report>,
    /// The Linux apps page (`brandmenu_linux.rs`).
    pub linux: LinuxPage,
    /// Cloud mode: entries left out of the menu and the About row (`set_cloud`).
    cloud: iyakki_agent::cloudmode::CloudMode,
}

impl Default for BrandMenu {
    fn default() -> BrandMenu {
        BrandMenu::new()
    }
}

impl BrandMenu {
    pub fn new() -> BrandMenu {
        BrandMenu { open: false, page: Page::Menu, sel: 0, scroll: 0, visible: 10, wrapped: None, about: None, players: Vec::new(), path: TextField::new("/path/to/a/video-or-audio-file", false), mac: None, mac_path: TextField::new("/mnt/usb/iyakki-wifi-firmware  or  /mnt/usb/macOS.dmg", false), mac_focus: false, mac_last: None, mac_busy: false, mac_leftover: 0, mac_cache: String::new(), mac_bt_held: false, mac_model: String::new(), mac_board: String::new(), note: None, anchor: (24.0, 76.0), admin: false, gpu: None, linux: LinuxPage::new(), cloud: iyakki_agent::cloudmode::CloudMode::desktop() }
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
        ENTRIES.iter().copied().filter(|e| (e.0 != Page::Developer || self.admin) && !self.cloud.hides_menu(cloud_id(e.0))).collect()
    }

    /// Cloud mode for this machine: hidden entries are neither listed nor reachable by key or jump.
    pub fn set_cloud(&mut self, c: iyakki_agent::cloudmode::CloudMode) {
        self.cloud = c;
        self.about = None;
        self.wrapped = None;
    }

    /// The About lines (made once; tests may give their own).
    pub fn set_about(&mut self, lines: Vec<(String, crate::about::Style)>) {
        self.about = Some(lines);
        self.wrapped = None;
    }

    fn about_lines(&mut self) -> &Vec<(String, crate::about::Style)> {
        let cloud = self.cloud.clone();
        self.about.get_or_insert_with(|| crate::about::this_machine_with(Some(&cloud)))
    }

    pub fn open_page(&mut self, p: Page) -> Outcome {
        if p == Page::Settings {
            return Outcome::Jump("settings", String::new());
        }
        if p == Page::Developer {
            return Outcome::Jump("settings", "tab:developer".into());
        }
        if p == Page::Cloud {
            return Outcome::Jump("cloud", String::new());
        }
        // the old Quick install page is gone: the entry (and its key q) opens the Store on its Setup tab
        if p == Page::Setup {
            return Outcome::Jump("store", "setup".into());
        }
        if self.cloud.hides_menu(cloud_id(p)) {
            return Outcome::Nothing;
        }
        self.page = p;
        self.sel = 0;
        self.scroll = 0;
        self.note = None;
        if p == Page::Linux {
            self.linux_reset();
        }
        if p == Page::About {
            self.about_lines();
        }
        if p == Page::Menu { Outcome::Nothing } else { Outcome::Opened(p) }
    }

    fn back(&mut self) -> Outcome {
        if self.page == Page::Gpu {
            return Outcome::Jump("store", "setup".into());
        }
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
            Page::Linux => self.linux_rows(),
            _ => 0,
        }
    }

    fn scroll_by(&mut self, d: i32) {
        let total = self.wrapped.as_ref().map_or(0, |w| w.1.len());
        let max = total.saturating_sub(self.visible.max(1));
        self.scroll = (self.scroll as i32 + d).clamp_safe(0, max as i32) as usize;
    }

    // ---- Mac drivers page

    fn mac_blocked(&mut self) -> bool {
        if self.mac_busy {
            self.note = Some(("The Mac firmware is being installed already: its progress is beside the clock in the top bar.".into(), false));
            return true;
        }
        false
    }

    fn mac_download(&mut self) -> Outcome {
        if self.mac_blocked() {
            return Outcome::Nothing;
        }
        self.note = None;
        Outcome::MacFirmware(iyakki_agent::macfw::Source::Download)
    }

    /// `Enter` in the field and the `f` key: a folder is read as a folder, a file as a disk image.
    fn mac_from_path(&mut self) -> Outcome {
        let p = self.mac_path.value.trim().to_string();
        let is_dir = std::path::Path::new(&p).is_dir();
        self.mac_path_source(!is_dir)
    }

    fn mac_path_source(&mut self, image: bool) -> Outcome {
        if self.mac_blocked() {
            return Outcome::Nothing;
        }
        let p = self.mac_path.value.trim().to_string();
        if p.is_empty() {
            self.mac_focus = true;
            self.note = Some(("Click the field and type where the files are: a folder or stick (the one the macOS script wrote, or a copy of usr/share/firmware) or a macOS disk image file.".into(), false));
            return Outcome::Nothing;
        }
        let path = std::path::Path::new(&p);
        if image && !path.is_file() {
            self.note = Some((format!("{p} is not a file. For a folder press From a folder or stick."), false));
            return Outcome::Nothing;
        }
        if !image && !path.is_dir() {
            self.note = Some((format!("{p} is not a folder. For a disk image file press From a disk image."), false));
            return Outcome::Nothing;
        }
        self.mac_focus = false;
        self.note = None;
        Outcome::MacFirmware(if image { iyakki_agent::macfw::Source::Image(p.into()) } else { iyakki_agent::macfw::Source::Folder(p.into()) })
    }

    fn mac_bt_load(&mut self) -> Outcome {
        if !self.mac_bt_held {
            self.note = Some(("No Bluetooth driver is set aside on this machine: nothing to load.".into(), false));
            return Outcome::Nothing;
        }
        Outcome::MacBtLoad
    }

    fn mac_copy_prompt(&mut self) -> Outcome {
        crate::ui::clipboard::copy(iyakki_agent::macfw::CHAT_PROMPT);
        self.note = Some(("Copied. Paste it into Claude Code, Copilot CLI (AI IDE tile) or the CHAT tile: each command still asks you first.".into(), true));
        Outcome::Nothing
    }

    pub fn handle_key(&mut self, k: &Key) -> Outcome {
        if matches!(k, Key::F(10)) {
            return Outcome::Close;
        }
        // the Linux apps page: its own keys and fields first
        if self.page == Page::Linux {
            if let Some(o) = self.linux_key(k) {
                return o;
            }
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
        // the Mac page's path field takes what is typed once it has been clicked; Esc lets go of it
        if self.page == Page::Mac && self.mac_focus {
            match k {
                Key::Esc => {
                    self.mac_focus = false;
                    return Outcome::Nothing;
                }
                Key::Enter => return self.mac_from_path(),
                other => {
                    self.mac_path.handle(other);
                    return Outcome::Nothing;
                }
            }
        }
        let n = self.rows();
        match k {
            Key::Esc | Key::Backspace | Key::Left => self.back(),
            Key::Down | Key::Tab if n > 0 => {
                self.sel = self.step_sel(1, n);
                Outcome::Nothing
            }
            Key::Up | Key::BackTab if n > 0 => {
                self.sel = self.step_sel(-1, n);
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
                Page::Gpu => Outcome::GpuTest,
                Page::Mac if self.mac_focus => self.mac_from_path(),
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
                    Page::Gpu if c == 't' => Outcome::GpuTest,
                    Page::Gpu if c == 'r' => Outcome::Opened(Page::Gpu),
                    Page::Gpu if c == 'i' => Outcome::Jump("store", "setup".into()),
                    Page::Mac if c == 'd' => Outcome::Jump("store", "drivers".into()),
                    Page::Mac if c == 'v' => Outcome::Jump("devices", String::new()),
                    Page::Mac if c == 'a' => self.mac_download(),
                    Page::Mac if c == 'f' => self.mac_from_path(),
                    Page::Mac if c == 'c' => self.mac_copy_prompt(),
                    Page::Mac if c == 'l' => self.mac_bt_load(),
                    _ => Outcome::Nothing,
                }
            }
            _ => Outcome::Nothing,
        }
    }

    /// A click on one of the panel's own regions.
    pub fn handle_hit(&mut self, ev: &Event) -> Outcome {
        if self.page == Page::Linux {
            if let Some(o) = self.linux_hit(ev) {
                return o;
            }
        }
        match ev {
            Event::Tab(CLOSE) => Outcome::Close,
            Event::Tab(BACK) => self.back(),
            Event::Tab(n) if (ENTRY..ENTRY + self.shown().len()).contains(n) => {
                self.sel = n - ENTRY;
                self.open_page(self.shown()[self.sel].0)
            }
            Event::Tab(PLAY) => self.handle_key(&Key::Enter),
            Event::Tab(STOP) => Outcome::StopMedia,
            Event::Tab(OPEN_MEDIA) => Outcome::Jump("media", String::new()),
            Event::Tab(INSTALL_PLAYER) => Outcome::Install(Item::Ffmpeg),
            Event::Tab(DRIVERS) => Outcome::Jump("store", "drivers".into()),
            Event::Tab(DEVICES) => Outcome::Jump("devices", String::new()),
            Event::Tab(GPU_TEST | GPU_REFRESH | GPU_INSTALL | GPU_PAGE) => self.gpu_hit(ev).unwrap_or(Outcome::Nothing),
            Event::Tab(MAC_DOWNLOAD) => self.mac_download(),
            Event::Tab(MAC_FOLDER) => self.mac_path_source(false),
            Event::Tab(MAC_IMAGE) => self.mac_path_source(true),
            Event::Tab(MAC_PROMPT) => self.mac_copy_prompt(),
            Event::Tab(MAC_BTLOAD) if self.mac_bt_held => Outcome::MacBtLoad,
            Event::Tab(MAC_CLEAN) if self.mac_leftover > 0 && !self.mac_busy => Outcome::MacClean,
            Event::FocusAt(MAC_PATH, x) => {
                self.mac_focus = true;
                self.mac_path.click(*x);
                Outcome::Nothing
            }
            Event::Paste(t) if self.page == Page::Mac && self.mac_focus => {
                self.mac_path.paste(t);
                Outcome::Nothing
            }
            Event::FocusAt(PATH_FIELD, x) => {
                self.path.click(*x);
                Outcome::Nothing
            }
            Event::Paste(t) if self.page == Page::Media => {
                self.path.paste(t);
                Outcome::Nothing
            }
            Event::Scroll(d) => {
                match self.page {
                    Page::About => self.scroll_by(-*d * 3),
                    // the sandbox / image-app list follows its selection: a notch moves it by a row (up is towards the start)
                    Page::Linux => self.linux_wheel(*d),
                    _ => {}
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
            Page::Media => 380.0 * s,
            _ => 640.0 * s,
        };
        let ph = want.min(h as f32 - ay - 12.0 * s).max(160.0 * s);
        let x = ax.clamp_safe(12.0 * s, (w as f32 - pw - 12.0 * s).max(12.0 * s));
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
        let title = if self.page == Page::Gpu { "GPU for local models" } else { ENTRIES.iter().find(|e| e.0 == self.page).map_or("IYAKKI", |e| e.1) };
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
            Page::Media => self.draw_media(g, body, s, tick),
            Page::Mac => self.draw_mac(g, body, s, tick),
            Page::Gpu => self.draw_gpu(g, body, s),
            Page::Linux => self.draw_linux(g, body, s, tick),
            Page::Settings | Page::Developer | Page::Setup | Page::Cloud => {}
        }
        // the foot: what just happened, else the keys
        let (foot, col) = match &self.note {
            Some((t, ok)) => (t.clone(), if *ok { OK } else { AMBER }),
            None => (
                match self.page {
                    Page::Menu => "Up/Down and Enter, or the letter · Esc or F10 closes",
                    Page::About => "Up/Down, PageUp/PageDown or the wheel scroll · Esc back",
                    Page::Media => "Type a path · Enter plays · Esc back",
                    Page::Gpu => "t or Enter: test prompt · r: refresh · i: Store \u{2192} Setup · Esc back",
                    Page::Linux => self.linux_foot(),
                    Page::Mac => "a: download from Apple · f: from the path · l: load Bluetooth anyway · c: copy the prompt · d: STORE \u{2192} Drivers · v: Devices · Esc back",
                    Page::Settings | Page::Developer | Page::Setup | Page::Cloud => "",
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
                Page::Setup => Icon::Store,
                Page::Media => Icon::Media,
                Page::Mac => Icon::Devices,
                Page::Developer => Icon::Ide,
                Page::Cloud => Icon::Mesh,
                Page::Linux => Icon::Cli,
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

    fn draw_mac(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        use crate::devpop::Level;
        let rows = mac_rows_for(self.mac.as_ref(), self.mac_model.is_empty() || iyakki_drivers::macboard::is_t2(&self.mac_model));
        // the buttons, laid out in lines that wrap at the panel's width; the page keeps them at the bottom whatever the size
        let size = 14.0 * s;
        let bh = size * 2.1 + 6.0 * s;
        let gap = 10.0 * s;
        let layout = |g: &mut Gfx, labels: &[&str]| -> Vec<(f32, f32)> {
            let (mut x, mut line) = (0.0f32, 0.0f32);
            labels
                .iter()
                .map(|l| {
                    let w = g.text_width(size, l, Face::Bold) + size * 1.7;
                    if x > 0.0 && x + w > body.w {
                        x = 0.0;
                        line += 1.0;
                    }
                    let at = (x, line);
                    x += w + gap;
                    at
                })
                .collect()
        };
        let mut actions: Vec<(String, [u8; 4], usize)> = vec![((if self.mac_cache.is_empty() { "Download from Apple" } else { "Retry (no new download)" }).into(), ACCENT, MAC_DOWNLOAD), ("From a folder or stick".into(), ACCENT, MAC_FOLDER), ("From a disk image".into(), ACCENT, MAC_IMAGE), ("Copy the prompt for Claude/Copilot".into(), MUTED, MAC_PROMPT)];
        if self.mac_bt_held {
            actions.push(("Load the Bluetooth driver anyway".into(), AMBER, MAC_BTLOAD));
        }
        if self.mac_leftover > 0 && !self.mac_busy {
            actions.push((format!("Delete download ({} MB)", self.mac_leftover >> 20), ERR, MAC_CLEAN));
        }
        let nav: [(&str, [u8; 4], usize); 2] = [("STORE \u{2192} Drivers", MUTED, DRIVERS), ("Devices", MUTED, DEVICES)];
        let act_pos = layout(g, &actions.iter().map(|a| a.0.as_str()).collect::<Vec<_>>());
        let nav_pos = layout(g, &nav.iter().map(|a| a.0).collect::<Vec<_>>());
        let act_lines = act_pos.last().map_or(0.0, |p| p.1 + 1.0);
        let nav_lines = nav_pos.last().map_or(0.0, |p| p.1 + 1.0);
        let notice_size = 11.5 * s;
        let notice = g.wrap(notice_size, &mac_notice(&self.mac_model, &self.mac_board), Face::Sans, body.w);
        let notice_lines = notice.len().min(3) as f32;
        let field_h = 36.0 * s;
        let fixed = nav_lines * bh + act_lines * bh + field_h + 8.0 * s + notice_lines * 15.0 * s + 6.0 * s;
        let top_end = body.y + body.h - fixed;
        // ---- the status rows and the last result, as far as they fit above
        let mut y = body.y;
        for r in &rows {
            if y + 40.0 * s > top_end {
                break;
            }
            let col = match r.level {
                Level::Ok => OK,
                Level::Warn => AMBER,
                Level::Missing => ERR,
                Level::Absent => DIM,
            };
            g.circle(body.x + 6.0 * s, y + 9.0 * s, 5.0 * s, col);
            g.text(body.x + 20.0 * s, y, 14.0 * s, TEXT, r.title, Face::Bold);
            y += 19.0 * s;
            let st = g.fit(12.0 * s, &r.state, Face::Sans, body.w - 20.0 * s);
            g.text(body.x + 20.0 * s, y, 12.0 * s, col, &st, Face::Sans);
            y += 17.0 * s;
            // what to do: only for the two rows this page can fix, and only while there is room
            if matches!(r.title, "Wi-Fi firmware" | "Bluetooth") && y + 32.0 * s < top_end {
                for l in g.wrap(11.5 * s, r.how, Face::Sans, body.w - 20.0 * s).iter().take(2) {
                    g.text(body.x + 20.0 * s, y, 11.5 * s, MUTED, l, Face::Sans);
                    y += 15.0 * s;
                }
            }
            y += 6.0 * s;
        }
        if let Some(o) = &self.mac_last {
            if y + 34.0 * s <= top_end {
                let (col, head) = if o.installed { (OK, o.headline.clone()) } else { (AMBER, o.headline.clone()) };
                for (i, l) in g.wrap(12.0 * s, &format!("Last run: {head}"), Face::Sans, body.w).iter().take(2).enumerate() {
                    g.text(body.x, y + i as f32 * 16.0 * s, 12.0 * s, col, l, Face::Sans);
                }
            }
            if !self.mac_cache.is_empty() && y + 34.0 * s + 32.0 * s <= top_end {
                for (i, l) in g.wrap(11.5 * s, &self.mac_cache, Face::Sans, body.w).iter().take(3).enumerate() {
                    g.text(body.x, y + 36.0 * s + i as f32 * 15.0 * s, 11.5 * s, MUTED, l, Face::Sans);
                }
            }
        } else if self.mac_busy && y + 18.0 * s <= top_end {
            g.text(body.x, y, 12.0 * s, ACCENT, "Installing the firmware: progress is beside the clock.", Face::Sans);
        }
        // ---- the foot: notice, field, source buttons, navigation
        let mut y = top_end;
        for l in notice.iter().take(3) {
            g.text(body.x, y, notice_size, DIM, l, Face::Sans);
            y += 15.0 * s;
        }
        y += 6.0 * s;
        let fr = R::new(body.x, y, body.w, field_h);
        self.mac_path.render(g, fr, self.mac_focus, s, (tick / 15) % 2 == 0, Face::Mono, MAC_PATH);
        y += field_h + 8.0 * s;
        for (a, p) in actions.iter().zip(&act_pos) {
            g.button(body.x + p.0, y + p.1 * bh, size, &a.0, a.1, Event::Tab(a.2));
        }
        y += act_lines * bh;
        for (a, p) in nav.iter().zip(&nav_pos) {
            g.button(body.x + p.0, y + p.1 * bh, size, a.0, a.1, Event::Tab(a.2));
        }
    }
}

#[cfg(test)]
#[path = "brandmenu_tests.rs"]
mod tests;
