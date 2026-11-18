//! The desktop parts an app can see: screens (outputs), views (named layouts of panes), panes, the dock and the wallpaper.
//! The dock and the wallpaper are reached through the ports [`DockPort`] and [`WallpaperPort`]: an SDK app never writes a file or talks to
//! the shell itself (the core rule). The shell side of each port goes through the core, which asks permission and logs; the in-memory
//! implementations here are what tests and the browser preview use.

use std::collections::BTreeMap;

use iyakki_uikit::look::{DockPos, Look};
use iyakki_uikit::wallpaper;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
}

/// One output: its size in pixels, its density, where it sits in the arrangement.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Device pixels per logical unit.
    pub scale: f32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

impl Screen {
    pub fn orientation(&self) -> Orientation {
        if self.height > self.width {
            Orientation::Portrait
        } else {
            Orientation::Landscape
        }
    }

    pub fn logical_size(&self) -> (f32, f32) {
        (self.width as f32 / self.scale, self.height as f32 / self.scale)
    }
}

/// The screens, and the per-screen choices of dock and wallpaper.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrangement {
    screens: Vec<Screen>,
    dock: BTreeMap<u32, DockPos>,
    wallpaper: BTreeMap<u32, wallpaper::Id>,
}

impl Arrangement {
    /// At least one screen; the first primary one stays primary (or the first screen becomes it).
    pub fn new(mut screens: Vec<Screen>) -> Arrangement {
        if screens.is_empty() {
            screens.push(Screen { id: 0, name: "screen".into(), width: 1280, height: 720, scale: 1.0, x: 0, y: 0, primary: true });
        }
        let first_primary = screens.iter().position(|s| s.primary).unwrap_or(0);
        for (i, s) in screens.iter_mut().enumerate() {
            s.primary = i == first_primary;
            if !(s.scale.is_finite() && s.scale > 0.0) {
                s.scale = 1.0;
            }
        }
        Arrangement { screens, dock: BTreeMap::new(), wallpaper: BTreeMap::new() }
    }

    pub fn single(w: u32, h: u32) -> Arrangement {
        Arrangement::new(vec![Screen { id: 0, name: "screen".into(), width: w, height: h, scale: 1.0, x: 0, y: 0, primary: true }])
    }

    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    pub fn primary(&self) -> &Screen {
        self.screens.iter().find(|s| s.primary).unwrap_or(&self.screens[0])
    }

    /// The screen with this id; a screen that was unplugged answers with the primary one, so a pane is never left without a screen.
    pub fn resolve(&self, id: u32) -> &Screen {
        self.screens.iter().find(|s| s.id == id).unwrap_or_else(|| self.primary())
    }

    pub fn screen_at(&self, x: i32, y: i32) -> Option<&Screen> {
        self.screens.iter().find(|s| x >= s.x && y >= s.y && x < s.x + s.width as i32 && y < s.y + s.height as i32)
    }

    /// Unplug a screen. The last screen cannot be removed. Its dock and wallpaper choices go with it.
    pub fn unplug(&mut self, id: u32) -> bool {
        if self.screens.len() <= 1 || !self.screens.iter().any(|s| s.id == id) {
            return false;
        }
        let was_primary = self.screens.iter().any(|s| s.id == id && s.primary);
        self.screens.retain(|s| s.id != id);
        if was_primary {
            self.screens[0].primary = true;
        }
        self.dock.remove(&id);
        self.wallpaper.remove(&id);
        true
    }

    /// The dock of a screen: the person's own choice for it, else the Look's position on the primary screen and **off** on any other
    /// (decision 2026-11-15: one dock per screen, off by default on secondary screens).
    pub fn dock_for(&self, look: &Look, screen: u32) -> DockPos {
        let s = self.resolve(screen);
        self.dock.get(&s.id).copied().unwrap_or(if s.primary { look.dock.position } else { DockPos::Off })
    }

    pub fn set_dock(&mut self, screen: u32, pos: DockPos) {
        let id = self.resolve(screen).id;
        self.dock.insert(id, pos);
    }

    /// The wallpaper of a screen: its own choice, else the Look's.
    pub fn wallpaper_for(&self, look: &Look, screen: u32) -> wallpaper::Id {
        self.wallpaper.get(&self.resolve(screen).id).copied().unwrap_or(look.wallpaper)
    }

    pub fn set_wallpaper(&mut self, screen: u32, id: wallpaper::Id) {
        let sid = self.resolve(screen).id;
        self.wallpaper.insert(sid, id);
    }
}

/// A named layout of panes (a desktop view): which one this is, out of how many.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewInfo {
    pub name: String,
    pub index: usize,
    pub count: usize,
}

/// Where an app is drawn right now: on which screen, in which view, how big (logical units). Moving the pane to another view or screen,
/// or resizing it, changes this and the host sends `UiEvent::Resized`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub screen: u32,
    pub view: String,
    pub w: f32,
    pub h: f32,
}

/// What the dock shows for an app: a badge number and one short status line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DockStatus {
    pub badge: Option<u32>,
    pub line: String,
}

/// The longest status line the dock shows.
pub const STATUS_MAX: usize = 40;

impl DockStatus {
    pub fn new(badge: Option<u32>, line: &str) -> DockStatus {
        DockStatus { badge, line: line.chars().filter(|c| !c.is_control()).take(STATUS_MAX).collect() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeskError {
    /// The person's policy does not let this person use that app.
    NotAllowed,
    UnknownApp,
    /// The core refused (a card was declined, the capability is not granted).
    Refused(String),
    /// This host has no such part (a browser preview without a dock).
    Unavailable,
}

/// The dock, as an app may use it. The shell's implementation asks the core.
pub trait DockPort {
    fn pinned(&self) -> Vec<String>;
    fn pin(&mut self, app: &str) -> Result<(), DeskError>;
    fn unpin(&mut self, app: &str) -> Result<(), DeskError>;
    /// Ask the shell to open another app.
    fn open(&mut self, app: &str) -> Result<(), DeskError>;
    /// Set the badge and status line of this app's own dock icon.
    fn set_status(&mut self, app: &str, status: DockStatus);
}

/// The wallpaper, as an app may use it (the picker widget calls this).
pub trait WallpaperPort {
    fn get(&self, screen: u32) -> wallpaper::Id;
    fn set(&mut self, screen: u32, id: wallpaper::Id) -> Result<(), DeskError>;
}

/// An in-memory dock: for tests and the browser preview.
#[derive(Default)]
pub struct MemDock {
    pub items: Vec<String>,
    pub known: Vec<String>,
    pub opened: Vec<String>,
    pub status: BTreeMap<String, DockStatus>,
}

impl MemDock {
    pub fn knowing(apps: &[&str]) -> MemDock {
        MemDock { known: apps.iter().map(|s| s.to_string()).collect(), ..MemDock::default() }
    }
}

impl DockPort for MemDock {
    fn pinned(&self) -> Vec<String> {
        self.items.clone()
    }
    fn pin(&mut self, app: &str) -> Result<(), DeskError> {
        if !self.known.iter().any(|k| k == app) {
            return Err(DeskError::UnknownApp);
        }
        if !self.items.iter().any(|k| k == app) {
            self.items.push(app.to_string());
        }
        Ok(())
    }
    fn unpin(&mut self, app: &str) -> Result<(), DeskError> {
        self.items.retain(|k| k != app);
        Ok(())
    }
    fn open(&mut self, app: &str) -> Result<(), DeskError> {
        if !self.known.iter().any(|k| k == app) {
            return Err(DeskError::UnknownApp);
        }
        self.opened.push(app.to_string());
        Ok(())
    }
    fn set_status(&mut self, app: &str, status: DockStatus) {
        self.status.insert(app.to_string(), status);
    }
}

/// An in-memory wallpaper store: one choice per screen.
#[derive(Default)]
pub struct MemWallpaper {
    pub chosen: BTreeMap<u32, wallpaper::Id>,
}

impl WallpaperPort for MemWallpaper {
    fn get(&self, screen: u32) -> wallpaper::Id {
        self.chosen.get(&screen).copied().unwrap_or_default()
    }
    fn set(&mut self, screen: u32, id: wallpaper::Id) -> Result<(), DeskError> {
        self.chosen.insert(screen, id);
        Ok(())
    }
}

/// The colour for text drawn straight on a wallpaper: the Look's text colour when it reads (4.5:1) on that wallpaper, else black or
/// white, whichever reads better. (Wallpapers are judged by their light/dark flag and a representative colour.)
pub fn text_over_wallpaper(look: &Look, id: wallpaper::Id) -> [u8; 4] {
    let bg: [u8; 4] = if id == wallpaper::Id::Theme { look.theme.bg_top } else if id.is_light() { [235, 240, 248, 255] } else { [24, 22, 48, 255] };
    let ratio = |c: [u8; 4]| iyakki_uikit::usertheme::contrast(c, bg);
    let own = look.theme.text;
    if ratio(own) >= 4.5 {
        return own;
    }
    let (black, white) = ([0, 0, 0, 255], [255, 255, 255, 255]);
    if ratio(black) >= ratio(white) {
        black
    } else {
        white
    }
}
