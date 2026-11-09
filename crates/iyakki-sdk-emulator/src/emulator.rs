//! The emulator core: a tile in the real WASM sandbox, driven by input events, with the fake services behind it. No window here; the
//! window (crates/iyakki-gui/src/sdk_emulate.rs) and the script runner both drive this type.

use crate::project::Project;
use crate::services::FakeServices;
use iyakki_tile_abi::{Color, DisplayList, Event, Op};
use iyakki_tile_host::render::{render, Image};
use iyakki_tile_host::{Budgets, TileRunner};
use std::path::PathBuf;
use std::sync::Arc;

/// The surface sizes the window offers (width, height in logical pixels).
pub const SIZES: [(i32, i32); 3] = [(800, 480), (1024, 600), (1920, 1080)];
const MAX_HISTORY: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A tap is press and release at one point, with no pointer movement before it.
    Touch,
    /// The pointer moves to the spot first, then presses and releases.
    Mouse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Tap(i32, i32),
    Move(i32, i32),
    Text(String),
    Key { code: u32, mods: u8 },
    Tick(u32),
    Hit(u32),
    Resize(i32, i32),
    Theme(bool),
}

pub fn theme_event(dark: bool) -> Event {
    if dark {
        Event::Theme { dark, accent: Color(70, 130, 230, 255), background: Color(18, 18, 22, 255), foreground: Color(235, 235, 240, 255) }
    } else {
        Event::Theme { dark, accent: Color(40, 110, 220, 255), background: Color(245, 245, 245, 255), foreground: Color(20, 20, 20, 255) }
    }
}

pub struct Emulator {
    pub project: Project,
    runner: TileRunner,
    pub services: Arc<FakeServices>,
    scratch: PathBuf,
    pub size: (i32, i32),
    pub mode: Mode,
    pub dark: bool,
    history: Vec<Input>,
    list: Option<DisplayList>,
    refused: Option<String>,
}

fn load_runner(p: &Project, svc: &Arc<FakeServices>) -> Result<TileRunner, String> {
    TileRunner::load(&p.code, p.manifest(), Budgets::default(), Some(svc.clone()), 1).map_err(|e| e.to_string())
}

impl Emulator {
    /// Load the tile, start it at `size` and draw the first frame. A tile that cannot load (wrong ABI, missing function) is refused with
    /// the same sentence the device would show.
    pub fn new(project: Project, scratch: PathBuf, size: (i32, i32), mode: Mode, dark: bool) -> Result<Emulator, String> {
        let services = Arc::new(FakeServices::new(scratch.clone(), project.config.clone()));
        let mut runner = load_runner(&project, &services)?;
        runner.init(size.0, size.1, 100, theme_event(dark)).map_err(|e| e.to_string())?;
        let mut e = Emulator { project, runner, services, scratch, size, mode, dark, history: Vec::new(), list: None, refused: None };
        e.refresh()?;
        Ok(e)
    }

    /// Why the tile was stopped (it used a capability its manifest does not declare), if it was. After that nothing is delivered.
    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    fn check_refused(&mut self) -> Result<(), String> {
        if self.refused.is_none() {
            if let Some(v) = self.services.violations().first() {
                self.refused = Some(format!("REFUSED: the tile called {v}, but its manifest does not declare it. Add \"{v}\" to tile.caps in iyakki-app.json (the install card will then show it), or remove the call."));
            }
        }
        match &self.refused {
            Some(m) => Err(m.clone()),
            None => Ok(()),
        }
    }

    fn refresh(&mut self) -> Result<(), String> {
        self.check_refused()?;
        let r = self.runner.frame();
        self.check_refused()?;
        self.list = Some(r.map_err(|e| e.to_string())?);
        Ok(())
    }

    fn deliver(&mut self, input: &Input) -> Result<(), String> {
        self.check_refused()?;
        if self.list.is_none() {
            self.refresh()?;
        }
        let r = match input {
            Input::Tap(x, y) => {
                if self.mode == Mode::Mouse {
                    self.runner.event(&[Event::PointerMove { x: *x, y: *y }]).map_err(|e| e.to_string())?;
                }
                self.runner.click(*x, *y).map(|_| true)
            }
            Input::Move(x, y) => self.runner.event(&[Event::PointerMove { x: *x, y: *y }]),
            Input::Text(t) => self.runner.event(&[Event::TextInput { text: t.clone() }]),
            Input::Key { code, mods } => self.runner.event(&[Event::Key { code: *code, mods: *mods, down: true }, Event::Key { code: *code, mods: *mods, down: false }]),
            Input::Tick(ms) => {
                self.services.advance(*ms as u64);
                self.runner.event(&[Event::Tick { dt_ms: *ms }])
            }
            Input::Hit(id) => self.runner.event(&[Event::Hit { id: *id }]),
            Input::Resize(w, h) => {
                self.size = (*w, *h);
                self.runner.event(&[Event::Resize { w: *w, h: *h, scale_pct: 100 }])
            }
            Input::Theme(d) => {
                self.dark = *d;
                self.runner.event(&[theme_event(*d)])
            }
        };
        self.check_refused()?;
        r.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Deliver one input and draw the next frame. The input is remembered so a reload can replay it.
    pub fn apply(&mut self, input: Input) -> Result<(), String> {
        self.deliver(&input)?;
        let full = self.history.len() >= MAX_HISTORY;
        match (self.history.last_mut(), &input) {
            // frame-by-frame ticks of the window are merged so the history stays short
            (Some(Input::Tick(a)), Input::Tick(b)) => *a = a.saturating_add(*b).min(60_000),
            _ if !full => self.history.push(input),
            _ => {}
        }
        self.refresh()
    }

    /// Start again at another size (the tile gets a fresh start, as if opened at that size).
    pub fn restart_at(&mut self, size: (i32, i32)) -> Result<(), String> {
        let p = self.project.clone();
        *self = Emulator::new(p, self.scratch.clone(), size, self.mode, self.dark)?;
        Ok(())
    }

    pub fn display_list(&self) -> Option<&DisplayList> {
        self.list.as_ref()
    }

    /// Every text the last frame draws.
    pub fn texts(&self) -> Vec<String> {
        self.list.iter().flat_map(|l| l.ops.iter()).filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
    }

    /// The last frame as a picture, drawn with the reference renderer.
    pub fn image(&self) -> Image {
        let t = theme_event(self.dark);
        let bg = if let Event::Theme { background, .. } = t { background } else { Color(0, 0, 0, 255) };
        render(self.list.as_ref().unwrap_or(&DisplayList::default()), self.size.0, self.size.1, bg)
    }

    /// Read the project again from disk and start the new code, replaying the inputs so far so the tile is where the developer left it.
    /// Returns a one-line status for the side panel.
    pub fn reload(&mut self) -> Result<String, String> {
        let project = Project::load(&self.project.source)?;
        let history = self.history.clone();
        let (size0, mode, dark) = (self.size, self.mode, self.dark);
        let mut fresh = Emulator::new(project, self.scratch.clone(), size0, mode, dark)?;
        let mut replayed = 0;
        for i in &history {
            if fresh.apply(i.clone()).is_err() {
                // the new code cannot follow the old inputs: start clean and say so
                let project = fresh.project.clone();
                *self = Emulator::new(project, self.scratch.clone(), size0, mode, dark)?;
                return Ok(format!("reloaded; the old inputs no longer fit the new code, so the tile started fresh ({replayed} of {} replayed)", history.len()));
            }
            replayed += 1;
        }
        *self = fresh;
        Ok(format!("reloaded; {replayed} earlier input(s) replayed"))
    }
}
