//! Laptop battery: what the machine's batteries and power adapter say, in plain words, and when to warn or shut down.
//!
//! Everything is read from `/sys/class/power_supply` (the kernel's own files: no process is started). [`read`] turns that folder
//! into one [`Reading`]; it is a pure function of a root folder ([`BatteryEnv`]), so tests feed it a made-up `/sys`.
//!
//! * A battery is shown ONLY when the machine has one: a supply of type `Battery` whose scope is not `Device` (a Bluetooth mouse
//!   or game pad reports a battery too, but it is not the computer's), and which is present. A desktop, a virtual machine and a
//!   Mac mini have none and show nothing. A laptop whose kernel has no battery driver (ACPI control-method battery `PNP0C0A`
//!   present, no `BAT*`) gets [`Presence::NoDriver`]: the indicator shows a question mark and the popup says what is missing.
//! * Several batteries (BAT0 + BAT1) are combined, weighted by their full capacity.
//! * Time left is computed here from `energy_now`/`power_now` (or `charge_now`/`current_now`), smoothed by [`Tracker`] and shown
//!   only when the numbers are sane.
//! * [`Alerter`] decides the warnings (20 %, 10 %, "plug in now" at 5 %) and the clean shutdown at the critical level. It never
//!   acts on a missing, zero or unknown reading, while charging or while a power adapter is connected, and it needs two readings
//!   in a row and a 30 second countdown that a cable cancels.
//!
//! See docs/BATTERY.md.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Seconds between readings while the shell is on screen, and when nobody is looking.
pub const POLL_ACTIVE_SECS: u64 = 10;
pub const POLL_IDLE_SECS: u64 = 60;
/// The visible countdown before a clean shutdown at the critical level.
pub const COUNTDOWN_SECS: u64 = 30;
/// At or below this many percent (and above zero) a discharging battery is critical.
pub const CRITICAL_PCT: f32 = 3.0;
/// Warning levels, in percent.
pub const WARN_LOW: f32 = 20.0;
pub const WARN_LOWER: f32 = 10.0;
pub const WARN_PLUG_IN: f32 = 5.0;
/// A level re-arms once the charge is this much above it again (no repeated warnings around a threshold).
const HYSTERESIS: f32 = 3.0;

/// The folder the readings come from: `/sys`, or a made-up one in tests.
#[derive(Clone, Debug)]
pub struct BatteryEnv {
    pub root: PathBuf,
}

impl BatteryEnv {
    pub fn real() -> BatteryEnv {
        BatteryEnv { root: PathBuf::from("/sys") }
    }
    pub fn at(root: impl Into<PathBuf>) -> BatteryEnv {
        BatteryEnv { root: root.into() }
    }
    fn supplies(&self) -> PathBuf {
        self.root.join("class/power_supply")
    }
}

impl Default for BatteryEnv {
    fn default() -> BatteryEnv {
        BatteryEnv::real()
    }
}

/// The three switches of Settings -> Battery (all on by default; stored as "off" flags so an old settings file means "on").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatterySettings {
    /// Hide the percentage next to the icon in the top bar.
    pub hide_percent: bool,
    /// No toasts at 20 % and 10 %, no "plug in now" card.
    pub no_warnings: bool,
    /// Do not shut down by itself at the critical level.
    pub no_shutdown: bool,
}

impl BatterySettings {
    pub fn show_percent(&self) -> bool {
        !self.hide_percent
    }
    pub fn warnings(&self) -> bool {
        !self.no_warnings
    }
    pub fn shutdown_at_critical(&self) -> bool {
        !self.no_shutdown
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

impl Status {
    fn parse(s: &str) -> Status {
        match s.trim().to_ascii_lowercase().as_str() {
            "charging" => Status::Charging,
            "discharging" => Status::Discharging,
            "full" => Status::Full,
            "not charging" => Status::NotCharging,
            _ => Status::Unknown,
        }
    }
}

/// The firmware's coarse level (`capacity_level`), used when there is no percentage and as a second opinion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Unknown,
    Critical,
    Low,
    Normal,
    High,
    Full,
}

impl Level {
    fn parse(s: &str) -> Level {
        match s.trim().to_ascii_lowercase().as_str() {
            "critical" => Level::Critical,
            "low" => Level::Low,
            "normal" => Level::Normal,
            "high" => Level::High,
            "full" => Level::Full,
            _ => Level::Unknown,
        }
    }
}

/// Whether the amounts are energy (µWh, µW) or charge (µAh, µA).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Energy,
    Charge,
}

/// One battery.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub name: String,
    pub percent: Option<f32>,
    pub status: Status,
    pub level: Level,
    pub kind: Kind,
    pub now: Option<f64>,
    pub full: Option<f64>,
    pub design: Option<f64>,
    /// Power or current right now (always positive; some drivers report it negative while discharging).
    pub rate: Option<f64>,
    pub voltage_v: Option<f64>,
    pub cycles: Option<u32>,
    pub technology: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// No battery of this computer (desktop, virtual machine, Mac mini): show nothing.
    None,
    /// A laptop battery exists (ACPI says so) but the kernel has no driver for it.
    NoDriver,
    Present,
}

/// What the indicator draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconState {
    /// Level not known (or no driver): a question mark.
    Unknown,
    Discharging,
    /// A bolt over the fill.
    Charging,
    /// On the adapter and full or not charging: a plug mark.
    Plugged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeKind {
    UntilEmpty,
    UntilFull,
}

/// Everything known about the computer's power at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub presence: Presence,
    pub cells: Vec<Cell>,
    /// Combined charge, 0 to 100; None when no battery gives one.
    pub percent: Option<f32>,
    pub status: Status,
    pub level: Level,
    /// Some power adapter is connected; None when the machine reports no adapter at all.
    pub ac_online: Option<bool>,
    /// Combined full capacity against design capacity, percent (at most 100).
    pub health_pct: Option<f32>,
    pub cycles: Option<u32>,
    pub technology: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    /// Smoothed combined power or current, in the combined unit (set by [`Tracker::apply`]).
    pub smooth_rate: Option<f64>,
    /// Seconds until empty or full (set by [`Tracker::apply`], only when sane).
    pub time_secs: Option<(TimeKind, u64)>,
    amounts: Option<Amounts>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Amounts {
    now: f64,
    full: f64,
    design: Option<f64>,
    rate: Option<f64>,
}

impl Default for Reading {
    fn default() -> Reading {
        Reading::none()
    }
}

impl Reading {
    /// No battery at all (a desktop).
    pub fn none() -> Reading {
        Reading { presence: Presence::None, cells: Vec::new(), percent: None, status: Status::Unknown, level: Level::Unknown, ac_online: None, health_pct: None, cycles: None, technology: None, manufacturer: None, model: None, smooth_rate: None, time_secs: None, amounts: None }
    }

    /// Whether the top bar and Settings show anything.
    pub fn shown(&self) -> bool {
        self.presence != Presence::None
    }

    pub fn readable(&self) -> bool {
        self.presence == Presence::Present
    }

    /// The percentage rounded for the bar ("85"), None when unknown.
    pub fn percent_whole(&self) -> Option<u32> {
        self.percent.map(|p| p.round().clamp(0.0, 100.0) as u32)
    }

    /// The icon state: question mark when the level is unknown, a bolt while charging, a plug on the adapter when full or not charging.
    pub fn icon_state(&self) -> IconState {
        if !self.readable() || (self.percent.is_none() && self.level == Level::Unknown) {
            return IconState::Unknown;
        }
        match self.status {
            Status::Charging => IconState::Charging,
            Status::Full | Status::NotCharging if self.ac_online != Some(false) => IconState::Plugged,
            _ => IconState::Discharging,
        }
    }

    /// The fill for the icon, 0 to 1 (a coarse guess from the firmware level when there is no percentage).
    pub fn fill_fraction(&self) -> f32 {
        match (self.percent, self.level) {
            (Some(p), _) => (p / 100.0).clamp(0.0, 1.0),
            (None, Level::Critical) => 0.05,
            (None, Level::Low) => 0.15,
            (None, Level::Normal) => 0.5,
            (None, Level::High) => 0.8,
            (None, Level::Full) => 1.0,
            (None, Level::Unknown) => 0.0,
        }
    }

    /// Below 10 % (red) or 20 % (amber) and not charging; the firmware's Critical and Low count when there is no percentage.
    pub fn low_band(&self) -> LowBand {
        if matches!(self.status, Status::Charging) {
            return LowBand::Normal;
        }
        match self.percent {
            Some(p) if p < 10.0 => LowBand::Red,
            Some(p) if p < 20.0 => LowBand::Amber,
            Some(_) => LowBand::Normal,
            None => match self.level {
                Level::Critical => LowBand::Red,
                Level::Low => LowBand::Amber,
                _ => LowBand::Normal,
            },
        }
    }

    /// "Charging", "Discharging", "Full", "Plugged in but not charging (battery protection)" ...
    pub fn status_words(&self) -> &'static str {
        match self.status {
            Status::Charging => "Charging",
            Status::Discharging => "Discharging",
            Status::Full => "Full",
            Status::NotCharging if self.ac_online == Some(true) => "Plugged in but not charging (battery protection)",
            Status::NotCharging => "Not charging",
            Status::Unknown if self.ac_online == Some(true) => "Plugged in",
            Status::Unknown => "Status unknown",
        }
    }

    /// "85%" or "?".
    pub fn percent_text(&self) -> String {
        self.percent_whole().map_or_else(|| "?".to_string(), |p| format!("{p}%"))
    }

    /// "About 2 h 10 min until empty" / "until full"; None unless the estimate is sane.
    pub fn time_words(&self) -> Option<String> {
        let (k, secs) = self.time_secs?;
        Some(format!("About {} {}", duration_words(secs), if k == TimeKind::UntilEmpty { "until empty" } else { "until full" }))
    }

    /// The rows the popup and Settings show: (label, value). Plain words; rows without data are left out.
    pub fn rows(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::new();
        if self.presence == Presence::NoDriver {
            v.push(("Battery".into(), NO_DRIVER_NOTE.into()));
            return v;
        }
        let charge = match (self.percent_whole(), self.level) {
            (Some(p), _) => format!("{p}%"),
            (None, Level::Unknown) => "Not reported".to_string(),
            (None, l) => format!("{} (no percentage reported)", level_words(l)),
        };
        v.push(("Charge".into(), charge));
        v.push(("Status".into(), self.status_words().into()));
        if let Some(t) = self.time_words() {
            v.push(("Time".into(), t));
        }
        if let Some(h) = self.health_pct {
            v.push(("Health".into(), format!("{:.0}% of its original capacity", h)));
        }
        if let Some(c) = self.cycles {
            v.push(("Charge cycles".into(), c.to_string()));
        }
        if let Some(t) = &self.technology {
            v.push(("Technology".into(), t.clone()));
        }
        if let Some(m) = &self.manufacturer {
            v.push(("Maker".into(), m.clone()));
        }
        if let Some(m) = &self.model {
            v.push(("Model".into(), m.clone()));
        }
        if self.cells.len() > 1 {
            v.push(("Batteries".into(), self.cells.len().to_string()));
        }
        v.push((
            "Power adapter".into(),
            match self.ac_online {
                Some(true) => "Connected",
                Some(false) => "Not connected",
                None => "Not reported",
            }
            .into(),
        ));
        v
    }

    /// The words for the tooltip and for screen readers: "Battery 85%, charging".
    pub fn summary(&self) -> String {
        if self.presence == Presence::NoDriver {
            return format!("Battery: {NO_DRIVER_NOTE}");
        }
        format!("Battery {}, {}", self.percent_text(), self.status_words().to_lowercase())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LowBand {
    Normal,
    Amber,
    Red,
}

pub const NO_DRIVER_NOTE: &str = "No battery driver: this image needs a newer kernel";

fn level_words(l: Level) -> &'static str {
    match l {
        Level::Critical => "Critically low",
        Level::Low => "Low",
        Level::Normal => "Normal",
        Level::High => "High",
        Level::Full => "Full",
        Level::Unknown => "Unknown",
    }
}

/// "2 h 10 min", "45 min", "under a minute".
pub fn duration_words(secs: u64) -> String {
    let mins = (secs + 30) / 60;
    match (mins / 60, mins % 60) {
        (0, 0) => "under a minute".to_string(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

// ---- reading /sys ------------------------------------------------------------------------------------------------------

fn text(p: &Path) -> Option<String> {
    let s = std::fs::read_to_string(p).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// A non-negative number from a file; negative, garbage and missing are None.
fn unsigned(p: &Path) -> Option<f64> {
    let v: i64 = text(p)?.parse().ok()?;
    (v >= 0).then_some(v as f64)
}

/// A number whose sign does not matter (current and power: some drivers report them negative while discharging).
fn magnitude(p: &Path) -> Option<f64> {
    let v: i64 = text(p)?.parse().ok()?;
    Some(v.unsigned_abs() as f64)
}

/// A short printable label from a file ("Li-ion"); None for empty, "Unknown" or odd text.
fn label(p: &Path) -> Option<String> {
    let t = text(p)?;
    let t: String = t.chars().filter(|c| !c.is_control()).collect();
    let t = t.trim();
    if t.is_empty() || t.len() > 40 || t.eq_ignore_ascii_case("unknown") || t == "0" {
        return None;
    }
    Some(t.to_string())
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Names the kernel gives the batteries of peripherals when it does not set `scope` (older kernels).
fn looks_like_peripheral(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["hid", "hidpp", "controller", "wacom", "mouse", "keyboard", "ps-", "gamepad", "joystick", "headset", "stylus", "pen"].iter().any(|k| n.contains(k))
}

fn cell_at(dir: &Path, name: &str) -> Option<Cell> {
    if text(&dir.join("type"))?.as_str() != "Battery" {
        return None;
    }
    match text(&dir.join("scope")).as_deref() {
        Some(s) if s.eq_ignore_ascii_case("device") => return None,
        None if looks_like_peripheral(name) => return None,
        _ => {}
    }
    if text(&dir.join("present")).as_deref() == Some("0") {
        return None;
    }
    let (kind, now, full, design, rate) = {
        let e = (unsigned(&dir.join("energy_now")), unsigned(&dir.join("energy_full")), unsigned(&dir.join("energy_full_design")), magnitude(&dir.join("power_now")));
        let c = (unsigned(&dir.join("charge_now")), unsigned(&dir.join("charge_full")), unsigned(&dir.join("charge_full_design")), magnitude(&dir.join("current_now")));
        if e.0.is_some() || e.1.is_some() { (Kind::Energy, e.0, e.1, e.2, e.3) } else { (Kind::Charge, c.0, c.1, c.2, c.3) }
    };
    // a full capacity of zero is garbage; so is a charge above the full capacity by a lot
    let full = full.filter(|f| *f > 0.0);
    let design = design.filter(|d| *d > 0.0);
    let now = match (now, full) {
        (Some(n), Some(f)) if n > f * 1.5 => None,
        (Some(n), Some(f)) => Some(n.min(f)),
        (n, _) => n,
    };
    let computed = match (now, full) {
        (Some(n), Some(f)) => Some((n / f * 100.0) as f32),
        _ => None,
    };
    let reported = unsigned(&dir.join("capacity")).filter(|c| *c <= 100.0).map(|c| c as f32);
    // a reported 0 that the amounts contradict is a driver that has not read the battery yet
    let percent = match (reported, computed) {
        (Some(r), Some(c)) if r == 0.0 && c > 5.0 => Some(c),
        (Some(r), _) => Some(r),
        (None, c) => c.map(|c| c.clamp(0.0, 100.0)),
    };
    let voltage = unsigned(&dir.join("voltage_now")).or_else(|| unsigned(&dir.join("voltage_min_design"))).filter(|v| *v > 0.0).map(|v| v / 1e6);
    let cycles = text(&dir.join("cycle_count")).and_then(|c| c.parse::<i64>().ok()).filter(|c| (1..=100_000).contains(c)).map(|c| c as u32);
    Some(Cell {
        name: name.to_string(),
        percent,
        status: text(&dir.join("status")).map_or(Status::Unknown, |s| Status::parse(&s)),
        level: text(&dir.join("capacity_level")).map_or(Level::Unknown, |s| Level::parse(&s)),
        kind,
        now,
        full,
        design,
        rate,
        voltage_v: voltage,
        cycles,
        technology: label(&dir.join("technology")),
        manufacturer: label(&dir.join("manufacturer")),
        model: label(&dir.join("model_name")),
    })
}

fn is_adapter_type(t: &str) -> bool {
    t == "Mains" || t == "UPS" || t.starts_with("USB")
}

/// Whether any power adapter (mains or a USB-C charger) is connected; None when the machine lists no adapter. One scan of the
/// folder and one small read per adapter: cheap enough to run every second while a countdown shows.
pub fn ac_online(env: &BatteryEnv) -> Option<bool> {
    let ps = env.supplies();
    let mut seen = false;
    let mut on = false;
    for n in names(&ps) {
        let d = ps.join(&n);
        let Some(t) = text(&d.join("type")) else { continue };
        if !is_adapter_type(&t) || text(&d.join("scope")).is_some_and(|s| s.eq_ignore_ascii_case("device")) {
            continue;
        }
        if let Some(o) = text(&d.join("online")) {
            seen = true;
            on |= o == "1";
        }
    }
    seen.then_some(on)
}

/// DMI chassis types of laptops, notebooks, tablets and the like (SMBIOS: 8 portable, 9 laptop, 10 notebook, 11 handheld,
/// 14 sub notebook, 30 tablet, 31 convertible, 32 detachable).
pub fn laptop_chassis(env: &BatteryEnv) -> bool {
    text(&env.root.join("class/dmi/id/chassis_type")).and_then(|t| t.parse::<u32>().ok()).is_some_and(|t| matches!(t, 8 | 9 | 10 | 11 | 14 | 30 | 31 | 32))
}

/// An ACPI control-method battery (`PNP0C0A`) that says a battery is inserted (or is on a laptop chassis).
fn acpi_battery_device(env: &BatteryEnv) -> bool {
    let dir = env.root.join("bus/acpi/devices");
    names(&dir).iter().filter(|n| n.starts_with("PNP0C0A")).any(|n| {
        // _STA bit 4: battery present
        match text(&dir.join(n).join("status")).and_then(|s| s.parse::<u32>().ok()) {
            Some(st) => st & 0x10 != 0 || laptop_chassis(env),
            None => laptop_chassis(env),
        }
    })
}

/// Read the machine's power state now.
pub fn read(env: &BatteryEnv) -> Reading {
    let ps = env.supplies();
    let cells: Vec<Cell> = names(&ps).iter().filter_map(|n| cell_at(&ps.join(n), n)).collect();
    let ac = ac_online(env);
    if cells.is_empty() {
        let mut r = Reading::none();
        if acpi_battery_device(env) {
            r.presence = Presence::NoDriver;
            r.ac_online = ac;
        }
        return r;
    }
    combine(cells, ac)
}

/// Amounts of every cell in one unit: as they are when all are energy (or all charge), else charge converted with the voltage.
fn common_amounts(cells: &[Cell]) -> Option<Vec<Amounts>> {
    let all_same = cells.iter().all(|c| c.kind == cells[0].kind);
    let mut out = Vec::new();
    for c in cells {
        let k = if all_same || c.kind == Kind::Energy { 1.0 } else { c.voltage_v? };
        out.push(Amounts { now: c.now? * k, full: c.full? * k, design: c.design.map(|d| d * k), rate: c.rate.map(|r| r * k) });
    }
    Some(out)
}

fn combine(cells: Vec<Cell>, ac: Option<bool>) -> Reading {
    let amounts = common_amounts(&cells).map(|v| {
        let full: f64 = v.iter().map(|a| a.full).sum();
        let now: f64 = v.iter().map(|a| a.now).sum();
        let design = if v.iter().all(|a| a.design.is_some()) { Some(v.iter().filter_map(|a| a.design).sum()) } else { None };
        // the rate is the sum of the cells that report one (a cell that is idle reports 0)
        let rate = if v.iter().any(|a| a.rate.is_some()) { Some(v.iter().filter_map(|a| a.rate).sum()) } else { None };
        Amounts { now, full, design, rate }
    });
    let percent = match amounts {
        Some(a) if a.full > 0.0 && cells.iter().all(|c| c.percent.is_some() && c.now.is_some()) => Some(((a.now / a.full * 100.0) as f32).clamp(0.0, 100.0)),
        _ => {
            // weighted by full capacity when known, else equally
            let known: Vec<(f32, f64)> = cells.iter().filter_map(|c| c.percent.map(|p| (p, c.full.unwrap_or(1.0)))).collect();
            let wsum: f64 = known.iter().map(|k| k.1).sum();
            if known.is_empty() || wsum <= 0.0 { None } else { Some((known.iter().map(|k| k.0 as f64 * k.1).sum::<f64>() / wsum) as f32) }
        }
    };
    let any = |s: Status| cells.iter().any(|c| c.status == s);
    let status = if any(Status::Charging) {
        Status::Charging
    } else if any(Status::Discharging) {
        Status::Discharging
    } else if cells.iter().all(|c| c.status == Status::Full) {
        Status::Full
    } else if any(Status::NotCharging) {
        Status::NotCharging
    } else if any(Status::Full) {
        Status::Full
    } else {
        Status::Unknown
    };
    // the lowest firmware level of any battery
    let level = cells.iter().map(|c| c.level).filter(|l| *l != Level::Unknown).min_by_key(|l| match l {
        Level::Critical => 0,
        Level::Low => 1,
        Level::Normal => 2,
        Level::High => 3,
        Level::Full => 4,
        Level::Unknown => 5,
    });
    let health_pct = amounts.and_then(|a| a.design.map(|d| (a.full / d * 100.0) as f32)).filter(|h| (1.0..=200.0).contains(h)).map(|h| h.min(100.0));
    let first = |f: fn(&Cell) -> Option<String>| cells.iter().find_map(f);
    Reading {
        presence: Presence::Present,
        percent,
        status,
        level: level.unwrap_or(Level::Unknown),
        ac_online: ac,
        health_pct,
        cycles: cells.iter().filter_map(|c| c.cycles).max(),
        technology: first(|c| c.technology.clone()),
        manufacturer: first(|c| c.manufacturer.clone()),
        model: first(|c| c.model.clone()),
        smooth_rate: None,
        time_secs: None,
        amounts,
        cells,
    }
}

// ---- time estimates -----------------------------------------------------------------------------------------------------

/// Smooths the power draw over readings and turns it into a time left.
#[derive(Debug, Default)]
pub struct Tracker {
    ema: Option<f64>,
    last: Option<(Status, Option<bool>)>,
}

/// Longest estimate shown, until empty and until full.
const MAX_EMPTY_SECS: f64 = 72.0 * 3600.0;
const MAX_FULL_SECS: f64 = 48.0 * 3600.0;
const MIN_SECS: f64 = 60.0;

impl Tracker {
    pub fn new() -> Tracker {
        Tracker::default()
    }

    /// Fill `smooth_rate` and `time_secs` of a reading. The average restarts when the status or the adapter changes.
    pub fn apply(&mut self, r: &mut Reading) {
        r.time_secs = None;
        r.smooth_rate = None;
        let key = (r.status, r.ac_online);
        if self.last != Some(key) {
            self.ema = None;
        }
        self.last = Some(key);
        let Some(a) = r.amounts else {
            return;
        };
        let rate = a.rate.filter(|x| x.is_finite() && *x > 0.0);
        match rate {
            Some(x) => self.ema = Some(self.ema.map_or(x, |e| e * 0.7 + x * 0.3)),
            None => return,
        }
        let Some(rate) = self.ema else { return };
        r.smooth_rate = Some(rate);
        let (kind, left, cap) = match r.status {
            Status::Discharging => (TimeKind::UntilEmpty, a.now, MAX_EMPTY_SECS),
            Status::Charging => (TimeKind::UntilFull, (a.full - a.now).max(0.0), MAX_FULL_SECS),
            _ => return,
        };
        // a draw below a fifth of a percent of the capacity an hour is a measuring error, not a draw
        if a.full <= 0.0 || rate / a.full < 0.002 {
            return;
        }
        let secs = left / rate * 3600.0;
        if secs.is_finite() && (MIN_SECS..=cap).contains(&secs) {
            r.time_secs = Some((kind, secs.round() as u64));
        }
    }
}

// ---- warnings and the critical shutdown ---------------------------------------------------------------------------------

/// What the shell should do after a reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// A toast: the charge is at 20 % or 10 %.
    Low(u8),
    /// The "Plug in now" card (5 %).
    PlugInNow,
    /// The visible countdown starts.
    CountdownStart,
    /// The countdown ended without a shutdown (plugged in, charging, data gone, switched off).
    CountdownCancelled,
    /// Shut down now, through the safe power-off path.
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertCfg {
    pub warnings: bool,
    pub shutdown: bool,
}

impl From<&BatterySettings> for AlertCfg {
    fn from(s: &BatterySettings) -> AlertCfg {
        AlertCfg { warnings: s.warnings(), shutdown: s.shutdown_at_critical() }
    }
}

#[derive(Debug, Default)]
pub struct Alerter {
    fired20: bool,
    fired10: bool,
    fired5: bool,
    streak: u8,
    countdown: Option<u64>,
    /// The person cancelled the countdown: no new one until the charge recovers or a cable is plugged in.
    suppressed: bool,
}

/// Discharging for real: the battery says so, no adapter says otherwise.
fn discharging(r: &Reading) -> bool {
    r.readable() && r.status == Status::Discharging && r.ac_online != Some(true)
}

/// The one place that decides "critical": discharging, and either a percentage from 1 to 3 or the firmware's own Critical.
/// A missing battery, an unknown status, a 0 with nothing else, a charging battery and a connected adapter are never critical.
pub fn is_critical(r: &Reading) -> bool {
    if !discharging(r) {
        return false;
    }
    match r.percent {
        Some(p) if p > 0.0 && p <= CRITICAL_PCT => true,
        _ => r.level == Level::Critical,
    }
}

impl Alerter {
    pub fn new() -> Alerter {
        Alerter::default()
    }

    /// Seconds left of a running countdown (`now` in seconds from any steady clock).
    pub fn countdown_left(&self, now: u64) -> Option<u64> {
        self.countdown.map(|s| COUNTDOWN_SECS.saturating_sub(now.saturating_sub(s)))
    }

    pub fn counting_down(&self) -> bool {
        self.countdown.is_some()
    }

    /// The person pressed Cancel.
    pub fn cancel_by_person(&mut self) {
        self.countdown = None;
        self.streak = 0;
        self.suppressed = true;
    }

    /// A new reading. `now` is seconds from a steady clock (not the wall clock, which can jump when the time is set).
    pub fn step(&mut self, r: &Reading, now: u64, cfg: AlertCfg) -> Vec<Alert> {
        let mut out = Vec::new();
        let p = r.percent;
        let on_power = r.status == Status::Charging || r.ac_online == Some(true) || matches!(r.status, Status::Full | Status::NotCharging);
        // re-arm: a cable, or the charge back above a level by a margin
        if on_power && r.readable() {
            self.fired20 = false;
            self.fired10 = false;
            self.fired5 = false;
            self.suppressed = false;
        }
        if let Some(p) = p {
            if p >= WARN_LOW + HYSTERESIS {
                self.fired20 = false;
            }
            if p >= WARN_LOWER + HYSTERESIS {
                self.fired10 = false;
            }
            if p >= WARN_PLUG_IN + HYSTERESIS {
                self.fired5 = false;
            }
            if p > CRITICAL_PCT + HYSTERESIS {
                self.suppressed = false;
            }
        }
        // warnings: once per level, only the deepest one when several were crossed between readings
        if cfg.warnings && discharging(r) {
            if let Some(p) = p.filter(|p| *p > 0.0) {
                if p <= WARN_PLUG_IN {
                    if !self.fired5 {
                        out.push(Alert::PlugInNow);
                    }
                    self.fired5 = true;
                    self.fired10 = true;
                    self.fired20 = true;
                } else if p <= WARN_LOWER {
                    if !self.fired10 {
                        out.push(Alert::Low(10));
                    }
                    self.fired10 = true;
                    self.fired20 = true;
                } else if p <= WARN_LOW {
                    if !self.fired20 {
                        out.push(Alert::Low(20));
                    }
                    self.fired20 = true;
                }
            }
        }
        // the critical shutdown
        let critical = is_critical(r) && cfg.shutdown && !self.suppressed;
        if critical {
            self.streak = self.streak.saturating_add(1);
            match self.countdown {
                None if self.streak >= 2 => {
                    self.countdown = Some(now);
                    out.push(Alert::CountdownStart);
                }
                Some(start) => {
                    let spent = now.saturating_sub(start);
                    if now < start || spent > COUNTDOWN_SECS + 10 {
                        // the clock jumped or the shell stalled: the person did not get the full 30 seconds, start again
                        self.countdown = Some(now);
                    } else if spent >= COUNTDOWN_SECS {
                        self.countdown = None;
                        self.streak = 0;
                        out.push(Alert::Shutdown);
                    }
                }
                None => {}
            }
        } else {
            self.streak = 0;
            if self.countdown.take().is_some() {
                out.push(Alert::CountdownCancelled);
            }
        }
        out
    }
}

// ---- tests --------------------------------------------------------------------------------------------------------------

#[cfg(test)]
#[path = "battery_tests.rs"]
mod tests;
