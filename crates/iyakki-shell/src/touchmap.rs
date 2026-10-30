//! Which display a touch screen belongs to, and where on that display a finger lands.
//!
//! A touch screen or digitizer reports raw positions over its own range (0..4095, say). The shell shows several displays as one
//! extended desktop, so the raw range must be mapped onto the rectangle of the display the panel belongs to, not onto the whole
//! desktop (that sent a tap on a second monitor to the built-in screen) and not onto the primary.
//!
//! The owner display of a device is decided in this order:
//! 1. an explicit choice kept per device (Settings -> Displays: "Touch screen: <device> -> <display>"), found by the key
//!    `vendor:product@port path`;
//! 2. a built-in (not USB) panel belongs to the built-in (eDP, LVDS, DSI) display;
//! 3. with one display, that display;
//! 4. an external USB digitizer belongs to the one external display when exactly one is connected (which USB hub shares a cable
//!    with which monitor is not knowable from the kernel, so this is a rule, said so in the log: "auto: only external display");
//! 5. otherwise the first external display is used for now and the log says "calibrate": the one-time "Touch the highlighted
//!    screen" step (Settings -> Displays) shows a prompt on one display at a time and learns which display the next tap belongs
//!    to, like Windows' tablet identification. The answer is kept by device key.
//!
//! Pure logic lives in [`TouchMap`]; [`shared`] is the one instance the input thread and the Settings page meet at.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// How a display is turned. The sizes of an [`OutRect`] are as seen (after the turn); the device's raw range is of the panel's
/// own orientation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rot {
    #[default]
    R0,
    /// Turned 90 degrees clockwise.
    R90,
    R180,
    /// Turned 90 degrees counter-clockwise.
    R270,
}

/// A display in the extended desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutRect {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub internal: bool,
    pub rot: Rot,
}

/// An absolute input device that is a touch screen or digitizer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TouchDev {
    /// `vendor:product@port path`, what a saved choice is kept under.
    pub key: String,
    /// `27c0:0859`.
    pub id: String,
    pub name: String,
    /// On the USB bus (an external monitor's digitizer), else built in.
    pub usb: bool,
    /// Speaks the multi-touch protocol (slots); else ABS_X/ABS_Y with BTN_TOUCH.
    pub mt: bool,
    /// Raw (min, max) of the horizontal and vertical position axes.
    pub x: (i32, i32),
    pub y: (i32, i32),
}

/// The answer of [`resolve`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// Index into the displays that were given.
    pub display: Option<usize>,
    /// Why: "saved choice", "auto: only external display", ...
    pub how: String,
}

pub fn resolve(dev: &TouchDev, outs: &[OutRect], saved: &[(String, String)]) -> Mapping {
    let pick = |i: usize, how: &str| Mapping { display: Some(i), how: how.to_string() };
    if outs.is_empty() {
        return Mapping { display: None, how: "no display".into() };
    }
    let mut stale = String::new();
    if let Some((_, want)) = saved.iter().find(|s| s.0 == dev.key) {
        match outs.iter().position(|o| &o.name == want) {
            Some(i) => return pick(i, "saved choice"),
            None => stale = format!("; saved display {want} is not connected"),
        }
    }
    let mut m = auto(dev, outs);
    m.how.push_str(&stale);
    m
}

fn auto(dev: &TouchDev, outs: &[OutRect]) -> Mapping {
    let pick = |i: usize, how: &str| Mapping { display: Some(i), how: how.to_string() };
    if outs.len() == 1 {
        return pick(0, "auto: only display");
    }
    if !dev.usb {
        if let Some(i) = outs.iter().position(|o| o.internal) {
            return pick(i, "auto: built-in panel");
        }
    }
    let ext: Vec<usize> = (0..outs.len()).filter(|i| !outs[*i].internal).collect();
    match ext.len() {
        1 => pick(ext[0], "auto: only external display"),
        0 => pick(0, "auto: first display"),
        _ => pick(ext[0], "auto: several external displays, first one used; run the touch calibration"),
    }
}

/// The pixel of the desktop for a raw position of `dev` on the display `o`.
pub fn map_point(dev: &TouchDev, o: &OutRect, rx: i32, ry: i32) -> (i32, i32) {
    let norm = |v: i32, r: (i32, i32)| -> f64 {
        let span = (r.1 - r.0).max(1) as f64;
        ((v - r.0) as f64 / span).clamp(0.0, 1.0)
    };
    let (fx, fy) = (norm(rx, dev.x), norm(ry, dev.y));
    let (sx, sy) = match o.rot {
        Rot::R0 => (fx, fy),
        Rot::R90 => (1.0 - fy, fx),
        Rot::R180 => (1.0 - fx, 1.0 - fy),
        Rot::R270 => (fy, 1.0 - fx),
    };
    (o.x + (sx * (o.w - 1).max(0) as f64).round() as i32, o.y + (sy * (o.h - 1).max(0) as f64).round() as i32)
}

// ------------------------------------------------------------------------------------------------ calibration

/// Seconds a display waits for its tap before the step goes on to the next display.
pub const STEP_MS: u64 = 12_000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Calib {
    names: Vec<String>,
    step: usize,
    deadline: u64,
    /// Devices already placed in this run (a device gets one display).
    placed: Vec<String>,
}

/// One touch device as the Settings page and the log show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevLine {
    pub key: String,
    pub id: String,
    pub name: String,
    pub display: Option<String>,
    pub how: String,
    /// The choice kept for it, if any.
    pub saved: Option<String>,
}

impl DevLine {
    /// The developer-mode log line: `touch: 27c0:0859 TouchScreen -> DP-4 (auto: only external display)`.
    pub fn log_line(&self) -> String {
        format!("touch: {} {} -> {} ({})", self.id, self.name, self.display.as_deref().unwrap_or("no display"), self.how)
    }
}

#[derive(Default)]
pub struct TouchMap {
    saved: Vec<(String, String)>,
    calib: Option<Calib>,
    lines: Vec<DevLine>,
    dir: Option<PathBuf>,
    gen: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct FileData {
    saved: Vec<(String, String)>,
}

const FILE: &str = "touch-map.json";

impl TouchMap {
    pub fn load(&mut self, dir: &Path) {
        self.dir = Some(dir.to_path_buf());
        self.saved = std::fs::read_to_string(dir.join(FILE)).ok().and_then(|t| serde_json::from_str::<FileData>(&t).ok()).unwrap_or_default().saved;
        self.gen += 1;
    }

    fn persist(&self) {
        if let Some(d) = &self.dir {
            let data = FileData { saved: self.saved.clone() };
            if let Err(e) = std::fs::write(d.join(FILE), serde_json::to_string_pretty(&data).unwrap_or_default()) {
                log::warn!("touch: could not keep the touch screen choices: {e}");
            }
        }
    }

    pub fn saved(&self) -> &[(String, String)] {
        &self.saved
    }

    /// Keep (or with `None` forget) the display a device belongs to.
    pub fn set_choice(&mut self, key: &str, display: Option<&str>) {
        self.saved.retain(|s| s.0 != key);
        if let Some(d) = display {
            self.saved.push((key.to_string(), d.to_string()));
        }
        self.gen += 1;
        self.persist();
    }

    /// Bumps whenever a choice or the calibration changes (the input thread then looks at the mapping again).
    pub fn generation(&self) -> u64 {
        self.gen
    }

    pub fn set_lines(&mut self, lines: Vec<DevLine>) {
        self.lines = lines;
    }

    pub fn lines(&self) -> &[DevLine] {
        &self.lines
    }

    pub fn start_calibration(&mut self, names: Vec<String>, now: u64) {
        if names.is_empty() {
            return;
        }
        self.calib = Some(Calib { names, step: 0, deadline: now + STEP_MS, placed: Vec::new() });
        self.gen += 1;
    }

    pub fn cancel_calibration(&mut self) {
        if self.calib.take().is_some() {
            self.gen += 1;
        }
    }

    /// The name of the display that shows the highlighted prompt now.
    pub fn calibrating(&self) -> Option<&str> {
        self.calib.as_ref().and_then(|c| c.names.get(c.step)).map(String::as_str)
    }

    fn advance(&mut self, now: u64) {
        if let Some(c) = &mut self.calib {
            c.step += 1;
            c.deadline = now + STEP_MS;
            if c.step >= c.names.len() {
                self.calib = None;
            }
            self.gen += 1;
        }
    }

    /// Nobody touched this display (Esc, or time ran out): on to the next.
    pub fn skip_calibration(&mut self, now: u64) {
        self.advance(now);
    }

    pub fn tick(&mut self, now: u64) {
        if self.calib.as_ref().is_some_and(|c| now >= c.deadline) {
            self.advance(now);
        }
    }

    /// A finger came down on the device `key` while calibrating: it belongs to the highlighted display. Returns
    /// `(device key, display)` when that was learned.
    pub fn tap(&mut self, key: &str, now: u64) -> Option<(String, String)> {
        let c = self.calib.as_ref()?;
        if c.placed.iter().any(|p| p == key) {
            return None; // already placed in this run
        }
        let name = c.names.get(c.step)?.clone();
        if let Some(c) = &mut self.calib {
            c.placed.push(key.to_string());
        }
        self.saved.retain(|s| s.0 != key);
        self.saved.push((key.to_string(), name.clone()));
        self.persist();
        self.advance(now);
        Some((key.to_string(), name))
    }
}

/// The map the input thread and the Settings page share.
pub fn shared() -> std::sync::MutexGuard<'static, TouchMap> {
    static S: OnceLock<Mutex<TouchMap>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(TouchMap::default())).lock().unwrap_or_else(|p| p.into_inner())
}

/// Milliseconds on a clock that only goes forward, the same on every thread.
pub fn now_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// Read the kept choices from the folder of the display files (once the shell starts driving the displays).
pub fn init(dir: &Path) {
    shared().load(dir);
}

/// Turn the output rectangles of the plan into what the mapper needs.
pub fn outs_of_plan(plan: &crate::display::outputs::Plan) -> Vec<OutRect> {
    plan.outputs.iter().map(|o| OutRect { name: o.name.clone(), x: o.x, y: o.y, w: o.w(), h: o.h(), internal: crate::display::outputs::is_internal(&o.name), rot: Rot::R0 }).collect()
}

/// Work out the mapping of every touch device against the displays, publish the lines for Settings and log the changes
/// (`logged` holds the last line of each device).
pub fn refresh(devs: &[std::sync::Arc<TouchDev>], outs: &[OutRect], logged: &mut Vec<(String, String)>) -> Vec<(String, Option<OutRect>)> {
    let mut m = shared();
    let saved = m.saved().to_vec();
    let mut lines = Vec::new();
    let mut targets = Vec::new();
    for d in devs {
        let r = resolve(d, outs, &saved);
        let line = DevLine {
            key: d.key.clone(),
            id: d.id.clone(),
            name: d.name.clone(),
            display: r.display.map(|i| outs[i].name.clone()),
            how: r.how,
            saved: saved.iter().find(|s| s.0 == d.key).map(|s| s.1.clone()),
        };
        let text = line.log_line();
        match logged.iter_mut().find(|l| l.0 == d.key) {
            Some(l) if l.1 == text => {}
            Some(l) => {
                log::info!("{text}");
                l.1 = text;
            }
            None => {
                log::info!("{text}");
                logged.push((d.key.clone(), text));
            }
        }
        targets.push((d.key.clone(), r.display.map(|i| outs[i].clone())));
        lines.push(line);
    }
    logged.retain(|l| devs.iter().any(|d| d.key == l.0));
    m.set_lines(lines);
    targets
}

/// Tests that use the shared map take this first (the map is one per process).
#[cfg(test)]
pub(crate) static SHARED_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
#[path = "touchmap_tests.rs"]
mod tests;
