//! The memory gate for panes and extra displays: they are switched on only when the machine has the memory for them.
//!
//! Pure functions over numbers (`MemTotal` and `MemAvailable`, swap and zram swap not counted: zram-backed pages are already
//! inside the kernel's accounting), plus the reader and the per-machine switch. The numbers and the reasoning are in
//! `docs/MULTI-DISPLAY.md` section 3.

use std::path::Path;

use iyakki_sec::policy::App as PApp;

pub const MIB: u64 = 1 << 20;
pub const GIB: u64 = 1 << 30;

/// Never use more than this share of `MemAvailable` for the screen features.
pub const BUDGET_SHARE_PCT: u64 = 25;
/// And always leave at least this much free.
pub const MIN_FREE: u64 = 512 * MIB;
/// A pane's own surface scratch (glyph cache, text shaping caches), added to its pixels.
pub const SURFACE_OVERHEAD: u64 = 3 * MIB;
/// Per live app instance, by class (estimates, see the doc; adjust after measuring on hardware).
pub const COST_LIGHT: u64 = 8 * MIB;
pub const COST_MEDIUM: u64 = 40 * MIB;
pub const COST_HEAVY: u64 = 200 * MIB;
/// Tiers by `MemTotal`: below `TIER_PANES2` one pane; below `TIER_PANES3` two; below `TIER_FULL` three panes and two displays.
pub const TIER_PANES2: u64 = GIB + GIB / 2;
pub const TIER_PANES3: u64 = 2 * GIB + GIB / 2;
pub const TIER_FULL: u64 = 3 * GIB + GIB / 2;
/// The most panes and displays at all.
pub const MAX_PANES: usize = 4;
pub const MAX_DISPLAYS: usize = 4;

/// How much an app instance weighs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppClass {
    Light,
    Medium,
    Heavy,
}

impl AppClass {
    pub fn cost(self) -> u64 {
        match self {
            AppClass::Light => COST_LIGHT,
            AppClass::Medium => COST_MEDIUM,
            AppClass::Heavy => COST_HEAVY,
        }
    }

    /// The smallest pane an app of this class is drawn in, before scaling.
    pub fn min_size(self) -> (f32, f32) {
        match self {
            AppClass::Light => (360.0, 240.0),
            AppClass::Medium => (480.0, 300.0),
            AppClass::Heavy => (640.0, 400.0),
        }
    }
}

pub fn class_of(app: PApp) -> AppClass {
    match app {
        PApp::Browser | PApp::Ide | PApp::Camera | PApp::Data | PApp::Dash | PApp::Flows | PApp::Media | PApp::Voice | PApp::LinuxWin | PApp::Lab => AppClass::Heavy,
        PApp::Checkup | PApp::Chat | PApp::Rag | PApp::Edit | PApp::Mcp | PApp::Cli | PApp::Store | PApp::Skills | PApp::SrePilot | PApp::Tales | PApp::Scan | PApp::Mesh | PApp::Runs | PApp::System | PApp::Usage | PApp::Calendar | PApp::Remote | PApp::Cameras | PApp::Pdf => AppClass::Medium,
        PApp::Users | PApp::Settings | PApp::Inbox | PApp::Radio | PApp::K8s | PApp::Disks | PApp::Git | PApp::Doctor | PApp::Learn | PApp::DevTools | PApp::Auth => AppClass::Light,
    }
}

/// What `/proc/meminfo` says, in bytes. `total` 0 means unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemFacts {
    pub total: u64,
    pub available: u64,
}

impl MemFacts {
    pub fn known(self) -> bool {
        self.total > 0
    }
}

/// Read memory from a root with a `proc/meminfo` (the real machine: `/`). Missing or garbled gives unknown.
pub fn read_mem(root: &Path) -> MemFacts {
    let r = iyakki_agent::sysinfo::SysRoots { proc: root.join("proc"), sys: root.join("sys"), passwd: root.join("etc/passwd") };
    let m = iyakki_agent::sysinfo::memory(&r);
    if m.total == 0 || m.available > m.total {
        return MemFacts::default();
    }
    MemFacts { total: m.total, available: m.available }
}

/// The real machine's memory.
pub fn read_mem_now() -> MemFacts {
    read_mem(Path::new("/"))
}

/// A display's mode in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    pub w: u32,
    pub h: u32,
}

impl DisplayMode {
    fn bytes(self) -> u64 {
        self.w as u64 * self.h as u64 * 4
    }
}

/// The per-machine switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Switch {
    /// Decided by memory (the default).
    #[default]
    Auto,
    /// Always on: tiers and budget are ignored (advanced; may be slow or run out of memory).
    On,
    /// Off: the shell behaves as before panes existed.
    Off,
}

impl Switch {
    pub fn label(self) -> &'static str {
        match self {
            Switch::Auto => "Automatic",
            Switch::On => "Always on (advanced, may be slow or run out of memory)",
            Switch::Off => "Off",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Switch::Auto => "auto",
            Switch::On => "on",
            Switch::Off => "off",
        }
    }
    pub fn from_id(s: &str) -> Switch {
        match s.trim() {
            "on" => Switch::On,
            "off" => Switch::Off,
            _ => Switch::Auto,
        }
    }
    pub fn next(self) -> Switch {
        match self {
            Switch::Auto => Switch::On,
            Switch::On => Switch::Off,
            Switch::Off => Switch::Auto,
        }
    }
}

const SWITCH_FILE: &str = "screen-features";

pub fn read_switch(dir: &Path) -> Switch {
    std::fs::read_to_string(dir.join(SWITCH_FILE)).map(|s| Switch::from_id(&s)).unwrap_or_default()
}

pub fn write_switch(dir: &Path, s: Switch) -> std::io::Result<()> {
    std::fs::write(dir.join(SWITCH_FILE), format!("{}\n", s.id()))
}

/// What the screen features may do right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub max_panes: usize,
    pub max_displays: usize,
    pub multi_display: bool,
    /// Why the answer is not the full one (empty when it is).
    pub reason: String,
    /// What the biggest allowed layout is estimated to cost, and the budget it was held against.
    pub cost: u64,
    pub budget: u64,
    pub switch: Switch,
}

impl Allowed {
    pub fn panes(&self) -> bool {
        self.max_panes >= 2
    }
}

/// How much the screen features may use: a quarter of what is available, and never into the last `MIN_FREE`.
pub fn budget(available: u64) -> u64 {
    (available / 100 * BUDGET_SHARE_PCT + available % 100 * BUDGET_SHARE_PCT / 100).min(available.saturating_sub(MIN_FREE))
}

/// The estimated cost of `panes` panes on `primary` and the extra `others` displays, with the app classes in `wanted` (the
/// first pane's app was running anyway and is not counted; panes without a class count as light).
pub fn layout_cost(primary: DisplayMode, panes: usize, wanted: &[AppClass], others: &[DisplayMode]) -> u64 {
    let mut cost = 0;
    if panes >= 2 {
        // the panes together cover the display; each also has its scratch
        cost += primary.bytes() + panes as u64 * SURFACE_OVERHEAD;
        for i in 1..panes {
            cost += wanted.get(i).copied().unwrap_or(AppClass::Light).cost();
        }
    }
    for d in others {
        cost += d.bytes() * 2 + SURFACE_OVERHEAD;
    }
    cost
}

fn tier(total: u64) -> (usize, usize) {
    if total == 0 || total < TIER_PANES2 {
        (1, 1)
    } else if total < TIER_PANES3 {
        (2, 1)
    } else if total < TIER_FULL {
        (3, 2)
    } else {
        (MAX_PANES, MAX_DISPLAYS)
    }
}

/// What panes and extra displays are allowed for this memory, these connected displays (the first is the primary) and the
/// app classes the person wants in the panes. Unknown memory gives the conservative answer.
pub fn screen_features(mem: MemFacts, displays: &[DisplayMode], wanted: &[AppClass]) -> Allowed {
    features_with(Switch::Auto, mem, displays, wanted)
}

/// [`screen_features`] under the per-machine switch.
pub fn features_with(switch: Switch, mem: MemFacts, displays: &[DisplayMode], wanted: &[AppClass]) -> Allowed {
    let single = |reason: String| Allowed { max_panes: 1, max_displays: 1, multi_display: false, reason, cost: 0, budget: budget(mem.available), switch };
    match switch {
        Switch::Off => return single("Screen features are off".into()),
        Switch::On => {
            let n = displays.len().clamp(1, MAX_DISPLAYS);
            return Allowed { max_panes: MAX_PANES, max_displays: MAX_DISPLAYS, multi_display: n >= 2, reason: "Always on: memory is not checked".into(), cost: 0, budget: budget(mem.available), switch };
        }
        Switch::Auto => {}
    }
    if !mem.known() {
        return single("memory could not be read".into());
    }
    let (tier_panes, tier_displays) = tier(mem.total);
    let budget = budget(mem.available);
    let primary = displays.first().copied().unwrap_or(DisplayMode { w: 1920, h: 1080 });
    let present = displays.len().max(1);
    // displays first: as many as the tier and the budget allow (the panes of the first one cost on top)
    let mut nd = present.min(tier_displays);
    let others = |n: usize| -> Vec<DisplayMode> { displays.iter().skip(1).take(n.saturating_sub(1)).copied().collect() };
    while nd > 1 && layout_cost(primary, 1, wanted, &others(nd)) > budget {
        nd -= 1;
    }
    let mut panes = tier_panes;
    while panes > 1 && layout_cost(primary, panes, wanted, &others(nd)) > budget {
        panes -= 1;
    }
    let cost = layout_cost(primary, panes, wanted, &others(nd));
    let mut why = Vec::new();
    if panes < MAX_PANES {
        if panes == tier_panes {
            why.push(format!("{} of memory in total allows {} pane{}", gb(mem.total), panes, if panes == 1 { "" } else { "s" }));
        } else {
            why.push(format!("only {} free allows {} pane{}", gb(mem.available), panes, if panes == 1 { "" } else { "s" }));
        }
    }
    if present > 1 && nd < present {
        why.push(if nd == tier_displays { format!("{} of memory in total allows {} display{}", gb(mem.total), nd, if nd == 1 { "" } else { "s" }) } else { format!("only {} free allows {} display{}", gb(mem.available), nd, if nd == 1 { "" } else { "s" }) });
    }
    Allowed { max_panes: panes, max_displays: nd.max(1), multi_display: nd >= 2, reason: why.join("; "), cost, budget, switch }
}

/// "4 GB" style: whole gigabytes, at least 1.
pub fn gb(bytes: u64) -> String {
    let g = (bytes as f64 / GIB as f64).round().max(1.0) as u64;
    format!("{g} GB")
}

fn gb1(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB as f64)
}

/// The line the picker and Settings -> Displays show.
pub fn memory_line(a: &Allowed, mem: MemFacts) -> String {
    match a.switch {
        Switch::Off => return "Screen features: off. This computer shows one app on one screen.".into(),
        Switch::On => return "Layouts: always on (memory is not checked; this may be slow or run out of memory)".into(),
        Switch::Auto => {}
    }
    if !mem.known() {
        return "Layouts: single screen only (memory could not be read)".into();
    }
    if a.max_panes <= 1 {
        let why = if a.reason.is_empty() { String::new() } else { format!(": {}", a.reason) };
        format!("Layouts: single screen only ({}){}", gb(mem.total), why)
    } else if a.max_panes < MAX_PANES {
        format!("Layouts: limited to {} panes ({}): {}", a.max_panes, gb(mem.total), a.reason)
    } else if !a.reason.is_empty() {
        format!("Layouts: available ({}, {} free): {}", gb(mem.total), gb1(mem.available), a.reason)
    } else {
        format!("Layouts: available ({}, {} free)", gb(mem.total), gb1(mem.available))
    }
}

/// Whether there is room to start an app of `class` into a pane now: what it costs must fit above the free floor.
pub fn room_for_app(mem: MemFacts, class: AppClass) -> bool {
    !mem.known() || mem.available >= MIN_FREE + class.cost()
}

#[cfg(test)]
#[path = "memgate_tests.rs"]
mod tests;
