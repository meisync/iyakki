//! The output manager: from what the card reports and what the person chose to a plan (which connector shows what, on which
//! CRTC, at which size and position), the pointer's way between outputs, hot-plug changes, the 15-second "Keep these settings?"
//! trial and the per-person file. Pure: no device is touched here (the card is a [`super::kms::DrmBackend`]).

use crate::safeclamp::SafeClamp;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::Path;

use super::kms::{Connector, ModeInfo, Resources};

/// A display as the shell sees it: a name, a size, and where it sits in the arrangement (for the "display on that side" keys).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispSpec {
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub x: i32,
    pub y: i32,
}

impl Plan {
    /// The displays of this plan, in plan order (primary first).
    pub fn specs(&self) -> Vec<DispSpec> {
        self.outputs.iter().map(|o| DispSpec { name: o.name.clone(), w: o.w() as u32, h: o.h() as u32, x: o.x, y: o.y }).collect()
    }
}

/// How long a new setting is tried before it reverts by itself.
pub const KEEP_SECS: u64 = 15;

/// Where an output sits relative to the primary one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    RightOf,
    LeftOf,
    Above,
    Below,
}

impl Placement {
    pub fn label(self) -> &'static str {
        match self {
            Placement::RightOf => "right of",
            Placement::LeftOf => "left of",
            Placement::Above => "above",
            Placement::Below => "below",
        }
    }
    pub fn next(self) -> Placement {
        match self {
            Placement::RightOf => Placement::Below,
            Placement::Below => Placement::LeftOf,
            Placement::LeftOf => Placement::Above,
            Placement::Above => Placement::RightOf,
        }
    }
}

/// What the person chose (kept per person, by output name).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub mirror: bool,
    pub primary: Option<String>,
    pub placement: Vec<(String, Placement)>,
    pub modes: Vec<(String, (u32, u32))>,
    /// The layout template of each output (`panes.rs` ids).
    pub templates: Vec<(String, String)>,
}

impl DisplayConfig {
    pub fn placement_of(&self, name: &str) -> Placement {
        self.placement.iter().find(|p| p.0 == name).map_or(Placement::RightOf, |p| p.1)
    }
    pub fn set_placement(&mut self, name: &str, p: Placement) {
        self.placement.retain(|x| x.0 != name);
        self.placement.push((name.to_string(), p));
    }
    pub fn mode_of(&self, name: &str) -> Option<(u32, u32)> {
        self.modes.iter().find(|p| p.0 == name).map(|p| p.1)
    }
    pub fn set_mode(&mut self, name: &str, m: (u32, u32)) {
        self.modes.retain(|x| x.0 != name);
        self.modes.push((name.to_string(), m));
    }
    pub fn template_of(&self, name: &str) -> Option<&str> {
        self.templates.iter().find(|p| p.0 == name).map(|p| p.1.as_str())
    }
    pub fn set_template(&mut self, name: &str, t: &str) {
        self.templates.retain(|x| x.0 != name);
        self.templates.push((name.to_string(), t.to_string()));
    }
}

/// One output of a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub name: String,
    pub connector: u32,
    pub crtc: u32,
    pub mode: ModeInfo,
    pub x: i32,
    pub y: i32,
}

impl Planned {
    pub fn w(&self) -> i32 {
        self.mode.hdisplay as i32
    }
    pub fn h(&self) -> i32 {
        self.mode.vdisplay as i32
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The primary first.
    pub outputs: Vec<Planned>,
    pub mirror: bool,
    /// Things the person should be told (an output left off, a mirror that fell back to extend).
    pub notes: Vec<String>,
}

fn is_internal(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("edp") || n.contains("lvds") || n.contains("dsi")
}

/// The mode to use at `size` (the one with the highest refresh rate), else the preferred one, else the biggest.
fn pick_mode(c: &Connector, want: Option<(u32, u32)>) -> Option<ModeInfo> {
    if let Some(sz) = want {
        if let Some(m) = c.modes.iter().filter(|m| m.size() == sz).max_by_key(|m| m.vrefresh) {
            return Some(*m);
        }
    }
    c.modes.iter().find(|m| m.preferred()).or_else(|| c.modes.iter().max_by_key(|m| m.hdisplay as u32 * m.vdisplay as u32)).copied()
}

/// Give every output its own CRTC: a matching (Kuhn's algorithm) in which an output keeps the CRTC it is on when it can.
fn assign_crtcs(res: &Resources, outs: &[&Connector]) -> Vec<Option<u32>> {
    let cands: Vec<Vec<usize>> = outs
        .iter()
        .map(|c| {
            let mut v: Vec<usize> = (0..res.crtcs.len()).filter(|i| c.possible_crtcs & (1 << i) != 0).collect();
            v.sort_by_key(|i| (res.crtcs[*i] != c.current_crtc, *i));
            v
        })
        .collect();
    let mut owner: Vec<Option<usize>> = vec![None; res.crtcs.len()];
    fn try_one(o: usize, cands: &[Vec<usize>], owner: &mut Vec<Option<usize>>, seen: &mut Vec<bool>) -> bool {
        for &c in &cands[o] {
            if seen[c] {
                continue;
            }
            seen[c] = true;
            if owner[c].is_none() || try_one(owner[c].unwrap_or(0), cands, owner, seen) {
                owner[c] = Some(o);
                return true;
            }
        }
        false
    }
    for o in 0..outs.len() {
        let mut seen = vec![false; res.crtcs.len()];
        try_one(o, &cands, &mut owner, &mut seen);
    }
    let mut out = vec![None; outs.len()];
    for (c, o) in owner.iter().enumerate() {
        if let Some(o) = o {
            out[*o] = Some(res.crtcs[c]);
        }
    }
    out
}

/// The sizes both outputs have in common, the biggest first.
fn common_sizes(cs: &[&Connector]) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = cs.first().map(|c| c.modes.iter().map(|m| m.size()).collect()).unwrap_or_default();
    v.sort();
    v.dedup();
    v.retain(|s| cs.iter().all(|c| c.modes.iter().any(|m| m.size() == *s)));
    v.sort_by_key(|s| std::cmp::Reverse(s.0 as u64 * s.1 as u64));
    v
}

/// Decide what to show where. `max_displays` is what the memory gate allows. Errors when nothing is connected.
pub fn plan(res: &Resources, conns: &[Connector], cfg: &DisplayConfig, max_displays: usize) -> Result<Plan, String> {
    let mut live: Vec<&Connector> = conns.iter().filter(|c| c.connected && !c.modes.is_empty()).collect();
    if live.is_empty() {
        return Err("no display is connected".into());
    }
    live.sort_by(|a, b| a.name.cmp(&b.name));
    let primary = cfg.primary.as_ref().and_then(|p| live.iter().position(|c| &c.name == p)).or_else(|| live.iter().position(|c| is_internal(&c.name))).unwrap_or(0);
    let p = live.remove(primary);
    live.insert(0, p);
    let mut notes = Vec::new();
    let allowed = max_displays.max(1);
    if live.len() > allowed {
        for c in &live[allowed..] {
            notes.push(format!("{} is left off: this computer has the memory for {} display{} only", c.name, allowed, if allowed == 1 { "" } else { "s" }));
        }
        live.truncate(allowed);
    }
    let mut mirror = cfg.mirror && live.len() > 1;
    let mut size_for: Vec<Option<(u32, u32)>> = live.iter().map(|c| cfg.mode_of(&c.name)).collect();
    if mirror {
        match common_sizes(&live).first() {
            Some(s) => size_for = vec![Some(*s); live.len()],
            None => {
                notes.push("Mirror needs a resolution both displays have; they share none, so they are extended instead".into());
                mirror = false;
            }
        }
    }
    let crtcs = assign_crtcs(res, &live);
    let mut outputs: Vec<Planned> = Vec::new();
    for (i, c) in live.iter().enumerate() {
        let (Some(crtc), Some(mode)) = (crtcs[i], pick_mode(c, size_for[i])) else {
            notes.push(format!("{} cannot be driven together with the others (no free CRTC)", c.name));
            continue;
        };
        outputs.push(Planned { name: c.name.clone(), connector: c.id, crtc, mode, x: 0, y: 0 });
    }
    if outputs.is_empty() {
        return Err("no display can be driven".into());
    }
    if !mirror {
        // the primary at the origin; the others stack outward on their side
        let (px, py, pw, ph) = (0, 0, outputs[0].w(), outputs[0].h());
        let (mut right, mut left, mut below, mut above) = (px + pw, px, py + ph, py);
        for o in outputs.iter_mut().skip(1) {
            match cfg.placement_of(&o.name) {
                Placement::RightOf => {
                    o.x = right;
                    o.y = py;
                    right += o.w();
                }
                Placement::LeftOf => {
                    left -= o.w();
                    o.x = left;
                    o.y = py;
                }
                Placement::Below => {
                    o.x = px;
                    o.y = below;
                    below += o.h();
                }
                Placement::Above => {
                    above -= o.h();
                    o.x = px;
                    o.y = above;
                }
            }
        }
        // the whole picture starts at (0, 0)
        let (minx, miny) = (outputs.iter().map(|o| o.x).min().unwrap_or(0), outputs.iter().map(|o| o.y).min().unwrap_or(0));
        for o in outputs.iter_mut() {
            o.x -= minx;
            o.y -= miny;
        }
    }
    Ok(Plan { outputs, mirror, notes })
}

impl Plan {
    /// The pointer's room: every output's rectangle.
    pub fn locate(&self, gx: i32, gy: i32) -> Option<(usize, i32, i32)> {
        self.outputs.iter().enumerate().find(|(_, o)| gx >= o.x && gx < o.x + o.w() && gy >= o.y && gy < o.y + o.h()).map(|(i, o)| (i, gx - o.x, gy - o.y))
    }

    /// The nearest point that is on an output (the pointer cannot leave the arrangement).
    pub fn clamp(&self, gx: i32, gy: i32) -> (i32, i32) {
        if self.locate(gx, gy).is_some() {
            return (gx, gy);
        }
        self.outputs
            .iter()
            .map(|o| {
                let (x, y) = (gx.clamp_safe(o.x, o.x + o.w() - 1), gy.clamp_safe(o.y, o.y + o.h() - 1));
                let d = ((x - gx) as i64).pow(2) + ((y - gy) as i64).pow(2);
                (d, x, y)
            })
            .min()
            .map_or((gx, gy), |(_, x, y)| (x, y))
    }

    /// Global position of a point of output `i`.
    pub fn global(&self, i: usize, lx: i32, ly: i32) -> (i32, i32) {
        self.outputs.get(i).map_or((lx, ly), |o| (o.x + lx, o.y + ly))
    }

    pub fn bounds(&self) -> (i32, i32) {
        (self.outputs.iter().map(|o| o.x + o.w()).max().unwrap_or(0), self.outputs.iter().map(|o| o.y + o.h()).max().unwrap_or(0))
    }
}

/// One change in what is plugged in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added(String),
    Removed(String),
    ModesChanged(String),
}

pub fn diff(old: &[Connector], new: &[Connector]) -> Vec<Change> {
    let live = |v: &[Connector]| -> Vec<(String, Vec<(u32, u32)>)> { v.iter().filter(|c| c.connected).map(|c| (c.name.clone(), c.modes.iter().map(|m| m.size()).collect())).collect() };
    let (o, n) = (live(old), live(new));
    let mut out = Vec::new();
    for (name, modes) in &n {
        match o.iter().find(|x| &x.0 == name) {
            None => out.push(Change::Added(name.clone())),
            Some(x) if &x.1 != modes => out.push(Change::ModesChanged(name.clone())),
            _ => {}
        }
    }
    for (name, _) in &o {
        if !n.iter().any(|x| &x.0 == name) {
            out.push(Change::Removed(name.clone()));
        }
    }
    out
}

/// A setting being tried: it reverts to `previous` unless kept in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    pub previous: DisplayConfig,
    pub until_ms: u64,
}

impl Trial {
    pub fn start(previous: DisplayConfig, now_ms: u64) -> Trial {
        Trial { previous, until_ms: now_ms + KEEP_SECS * 1000 }
    }
    pub fn secs_left(&self, now_ms: u64) -> u64 {
        self.until_ms.saturating_sub(now_ms).div_ceil(1000)
    }
    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.until_ms
    }
}

// ------------------------------------------------------------------------------------------------ the person's file

fn file(dir: &Path, user: &str) -> std::path::PathBuf {
    dir.join(format!("displays-{user}.json"))
}

pub fn load(dir: &Path, user: &str) -> DisplayConfig {
    if !iyakki_sec::policy::valid_username(user) {
        return DisplayConfig::default();
    }
    std::fs::read_to_string(file(dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(dir: &Path, user: &str, cfg: &DisplayConfig) -> std::io::Result<()> {
    if !iyakki_sec::policy::valid_username(user) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad user name"));
    }
    std::fs::write(file(dir, user), serde_json::to_string_pretty(cfg).unwrap_or_default())
}

// ------------------------------------------------------------------------------------------------ the hub

/// What the Settings page asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Apply(DisplayConfig),
    Keep,
    Revert,
    Identify,
}

/// An output as the Settings page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutView {
    pub name: String,
    pub connected: bool,
    /// (w, h, hz), the preferred first.
    pub modes: Vec<(u32, u32, u32)>,
    pub current: Option<(u32, u32)>,
    pub pos: (i32, i32),
    pub primary: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Whether the shell drives the displays itself (KMS) or uses the framebuffer.
    pub kms: bool,
    /// Why not, in one line (empty when it does).
    pub why_not: String,
    pub outputs: Vec<OutView>,
    pub config: DisplayConfig,
    pub trial_secs: Option<u64>,
    pub notes: Vec<String>,
}

#[derive(Default)]
pub struct Hub {
    status: RefCell<Status>,
    requests: RefCell<Vec<Request>>,
    identify_until_ms: RefCell<u64>,
}

impl Hub {
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }
    pub fn set_status(&self, s: Status) {
        *self.status.borrow_mut() = s;
    }
    pub fn request(&self, r: Request) {
        self.requests.borrow_mut().push(r);
    }
    pub fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
    pub fn identify(&self, until_ms: u64) {
        *self.identify_until_ms.borrow_mut() = until_ms;
    }
    pub fn identifying(&self, now_ms: u64) -> bool {
        now_ms < *self.identify_until_ms.borrow()
    }
}

thread_local! {
    static HUB: std::rc::Rc<Hub> = std::rc::Rc::new(Hub::default());
}

/// The hub of the thread that runs the shell (the Settings page and the display loop meet here).
pub fn hub() -> std::rc::Rc<Hub> {
    HUB.with(|h| h.clone())
}

/// Milliseconds on a clock that only goes forward.
pub fn now_ms() -> u64 {
    thread_local! {
        static START: std::time::Instant = std::time::Instant::now();
    }
    START.with(|s| s.elapsed().as_millis() as u64)
}

#[cfg(test)]
#[path = "outputs_tests.rs"]
mod tests;
