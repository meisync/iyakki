//! A connected monitor whose mode list is wrong. On a Mac mini 2018 (kernel 6.12, i915) a 4K monitor plugged in by USB-C for the
//! first time was `connected` and its `edid` file held a good 256-byte EDID (preferred timing 3840x2160, 4K modes in the CTA block),
//! but the `modes` file listed only the generic fallback set (1920x1080 and smaller): the kernel had pruned or not yet read the
//! modes when the link came up. A second plug of the same cable was fine. Nothing told the shell to ask again.
//!
//! This module decides from plain files whether a connector looks like that ([`assess`]: the EDID's preferred timing, or a larger
//! CTA video mode, is missing from the list; the EDID is empty or short; the list is empty), and asks the kernel to look again by
//! writing `detect` to the connector's `status` file ([`recover`]), a few times with a growing delay, never on a screen thread
//! ([`recover_in_background`]). Everything takes the sysfs folder as a parameter, so the tests use a fake one.

use std::path::Path;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// The waits after each `detect`: one second, three, eight.
pub const RETRY_DELAYS: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(3), Duration::from_secs(8)];

/// Sizes the kernel offers for a monitor it knows nothing about (the no-EDID fallback set, and the old VESA ones near it).
const FALLBACK_SIZES: [(u32, u32); 18] = [
    (640, 480),
    (720, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 768),
    (1280, 800),
    (1280, 960),
    (1280, 1024),
    (1360, 768),
    (1366, 768),
    (1400, 1050),
    (1440, 900),
    (1600, 900),
    (1600, 1200),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
];

/// What an EDID says about sizes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdidInfo {
    /// The first detailed timing of the base block: what the monitor wants to run at (w, h, hz rounded).
    pub preferred: Option<(u32, u32, u32)>,
    /// Every size the EDID names: detailed timings of every block, and the video data block of the CTA extensions. Biggest first.
    pub sizes: Vec<(u32, u32)>,
}

/// The size of a CTA-861 video identification code (the common ones: up to 4K and 4096x2160).
pub fn vic_size(vic: u8) -> Option<(u32, u32)> {
    Some(match vic {
        1 => (640, 480),
        2 | 3 | 6 | 7 | 17 | 18 => (720, 480),
        4 | 19 | 41 | 47 | 60..=62 | 65..=71 => (1280, 720),
        5 | 16 | 20 | 31..=34 | 39 | 40 | 63 | 64 | 72..=78 => (1920, 1080),
        93..=97 => (3840, 2160),
        98..=102 => (4096, 2160),
        _ => return None,
    })
}

/// One 18-byte detailed timing descriptor: (width, height, refresh rounded). `None` for a descriptor that is not a timing (pixel clock 0).
fn dtd(d: &[u8]) -> Option<(u32, u32, u32)> {
    if d.len() < 18 {
        return None;
    }
    let clock = u32::from(u16::from_le_bytes([d[0], d[1]])) * 10_000;
    if clock == 0 {
        return None;
    }
    let (ha, hb) = (u32::from(d[2]) | (u32::from(d[4] & 0xf0) << 4), u32::from(d[3]) | (u32::from(d[4] & 0x0f) << 8));
    let (va, vb) = (u32::from(d[5]) | (u32::from(d[7] & 0xf0) << 4), u32::from(d[6]) | (u32::from(d[7] & 0x0f) << 8));
    let total = u64::from(ha + hb) * u64::from(va + vb);
    if ha == 0 || va == 0 || total == 0 {
        return None;
    }
    Some((ha, va, ((u64::from(clock) + total / 2) / total) as u32))
}

/// Parse the base block's detailed timing descriptors and the CTA extensions. `None` when this is not an EDID (short or no header).
pub fn parse_edid(edid: &[u8]) -> Option<EdidInfo> {
    const HEADER: [u8; 8] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }
    let mut info = EdidInfo::default();
    for (i, off) in [54usize, 72, 90, 108].into_iter().enumerate() {
        if let Some(t) = dtd(&edid[off..off + 18]) {
            if i == 0 {
                info.preferred = Some(t);
            }
            info.sizes.push((t.0, t.1));
        }
    }
    for block in edid[128..].chunks_exact(128) {
        if block[0] != 0x02 {
            continue; // not CTA-861
        }
        let end = usize::from(block[2]).clamp(4, 127);
        let mut p = 4;
        while p < end {
            let (tag, len) = (block[p] >> 5, usize::from(block[p] & 0x1f));
            if p + 1 + len > 127 {
                break;
            }
            if tag == 2 {
                for v in &block[p + 1..p + 1 + len] {
                    // bit 7 marks a native mode, but only for the codes 1..=64
                    let vic = if *v & 0x7f <= 64 { *v & 0x7f } else { *v };
                    if let Some(s) = vic_size(vic) {
                        info.sizes.push(s);
                    }
                }
            }
            p += 1 + len;
        }
        let mut p = end;
        while p >= 4 && p + 18 <= 127 {
            match dtd(&block[p..p + 18]) {
                Some(t) => info.sizes.push((t.0, t.1)),
                None => break,
            }
            p += 18;
        }
    }
    info.sizes.sort_by_key(|s| std::cmp::Reverse(u64::from(s.0) * u64::from(s.1)));
    info.sizes.dedup();
    Some(info)
}

/// What is wrong with a connected connector, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    /// The list holds what the EDID asks for.
    Fine,
    /// The `edid` file is empty, short or not an EDID.
    NoEdid,
    /// Connected, but the `modes` file is empty.
    NoModes,
    /// The EDID's preferred size is not in the list.
    MissingPreferred(u32, u32),
    /// Only fallback sizes are listed while the EDID names a bigger one.
    FallbackOnly(u32, u32),
}

impl Health {
    pub fn is_fine(&self) -> bool {
        *self == Health::Fine
    }

    pub fn describe(&self) -> String {
        match self {
            Health::Fine => "fine".into(),
            Health::NoEdid => "no usable EDID".into(),
            Health::NoModes => "no modes".into(),
            Health::MissingPreferred(w, h) => format!("preferred {w}x{h} missing from the modes"),
            Health::FallbackOnly(w, h) => format!("only fallback modes while the EDID names {w}x{h}"),
        }
    }
}

/// The size in a `modes` line ("3840x2160", "1920x1080i", "2560x1600p-60").
fn mode_size(line: &str) -> Option<(u32, u32)> {
    let (w, rest) = line.trim().split_once('x')?;
    let h: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let (w, h) = (w.trim().parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Judge one connector from its `edid` bytes and its `modes` text. Pure.
pub fn assess(edid: &[u8], modes_text: &str) -> Health {
    let Some(info) = parse_edid(edid) else { return Health::NoEdid };
    let modes: Vec<(u32, u32)> = modes_text.lines().filter_map(mode_size).collect();
    if modes.is_empty() {
        return Health::NoModes;
    }
    if let Some((w, h, _)) = info.preferred {
        if !modes.contains(&(w, h)) {
            return Health::MissingPreferred(w, h);
        }
    }
    let area = |s: &(u32, u32)| u64::from(s.0) * u64::from(s.1);
    if let (Some(best), Some(listed)) = (info.sizes.iter().max_by_key(|s| area(s)), modes.iter().max_by_key(|s| area(s))) {
        if area(best) > area(listed) && modes.iter().all(|m| FALLBACK_SIZES.contains(m)) {
            return Health::FallbackOnly(best.0, best.1);
        }
    }
    Health::Fine
}

/// A connected connector and how its mode list looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// `card0-DP-1`.
    pub sysfs: String,
    pub health: Health,
    /// Identifies the state (EDID and modes text), so a recovery that gave up is not repeated for the same state.
    pub signature: u64,
}

impl Check {
    /// `DP-1`.
    pub fn short(&self) -> &str {
        self.sysfs.split_once('-').map_or(self.sysfs.as_str(), |(_, n)| n)
    }
}

fn hash(edid: &[u8], modes: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in edid.iter().chain(modes.as_bytes()) {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Every connected connector of a GPU card (`cardN-<connector>` folders of `sys_drm`) with its health.
pub fn check_all(sys_drm: &Path) -> Vec<Check> {
    let mut names: Vec<String> = std::fs::read_dir(sys_drm).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    names.sort();
    let mut out = Vec::new();
    for n in names {
        let Some((card, _)) = n.split_once('-') else { continue };
        if !card.strip_prefix("card").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        let dir = sys_drm.join(&n);
        if std::fs::read_to_string(dir.join("status")).map(|s| s.trim() == "connected").ok() != Some(true) {
            continue;
        }
        // a firmware screen (simpledrm and the like) has neither an EDID nor a mode file: not a monitor we can ask again
        if !dir.join("edid").exists() && !dir.join("modes").exists() {
            continue;
        }
        let edid = std::fs::read(dir.join("edid")).unwrap_or_default();
        let modes = std::fs::read_to_string(dir.join("modes")).unwrap_or_default();
        out.push(Check { sysfs: n, health: assess(&edid, &modes), signature: hash(&edid, &modes) });
    }
    out
}

/// Ask the kernel to detect this connector again (and read its EDID and modes again).
pub fn redetect(sys_drm: &Path, sysfs: &str) -> std::io::Result<()> {
    std::fs::write(sys_drm.join(sysfs).join("status"), "detect")
}

/// How a recovery went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub connector: String,
    /// How many `detect` rounds were made.
    pub attempts: u32,
    /// The mode list is right now.
    pub fixed: bool,
}

static GAVE_UP: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

fn gave_up_on(c: &Check) -> bool {
    GAVE_UP.lock().map(|g| g.iter().any(|(n, s)| *n == c.sysfs && *s == c.signature)).unwrap_or(false)
}

fn remember_give_up(c: &Check) {
    if let Ok(mut g) = GAVE_UP.lock() {
        g.retain(|(n, _)| *n != c.sysfs);
        g.push((c.sysfs.clone(), c.signature));
    }
}

fn forget_give_ups() {
    if let Ok(mut g) = GAVE_UP.lock() {
        g.clear();
    }
}

/// For every connected connector that looks wrong: write `detect`, wait `delays[i]`, look again; stop at the first look that is
/// right. `force` (the person pressed "Look again") ignores earlier give-ups. A connector that stays wrong after the last delay is
/// remembered, so the events its own writes cause do not start it over; a change of its EDID or modes does. `wait` sleeps (the
/// tests pass one that only counts), `log` gets one `iyakki.display_retry` line per attempt. Returns what was tried.
pub fn recover(sys_drm: &Path, why: &str, force: bool, delays: &[Duration], wait: &mut dyn FnMut(Duration), log: &mut dyn FnMut(&str)) -> Vec<Recovery> {
    if force {
        forget_give_ups();
    }
    let mut pending: Vec<Check> = check_all(sys_drm).into_iter().filter(|c| !c.health.is_fine() && !gave_up_on(c)).collect();
    let mut out: Vec<Recovery> = pending.iter().map(|c| Recovery { connector: c.short().to_string(), attempts: 0, fixed: false }).collect();
    let tried: Vec<String> = pending.iter().map(|c| c.sysfs.clone()).collect();
    for (i, delay) in delays.iter().enumerate() {
        if pending.is_empty() {
            break;
        }
        for c in &pending {
            if let Err(e) = redetect(sys_drm, &c.sysfs) {
                log(&format!("iyakki.display_retry connector={} why=\"{why}\" attempt={}/{} could not write detect: {e}", c.short(), i + 1, delays.len()));
            }
            if let Some(o) = out.iter_mut().find(|o| o.connector == c.short()) {
                o.attempts += 1;
            }
        }
        wait(*delay);
        let now = check_all(sys_drm);
        for c in &pending {
            let after = now.iter().find(|n| n.sysfs == c.sysfs);
            log(&format!(
                "iyakki.display_retry connector={} why=\"{why}\" attempt={}/{} waited_ms={} before=\"{}\" after=\"{}\"",
                c.short(),
                i + 1,
                delays.len(),
                delay.as_millis(),
                c.health.describe(),
                after.map_or("disconnected".to_string(), |a| a.health.describe())
            ));
            if after.is_some_and(|a| a.health.is_fine()) {
                if let Some(o) = out.iter_mut().find(|o| o.connector == c.short()) {
                    o.fixed = true;
                }
            }
        }
        // still connected and still wrong: another round
        pending = now.into_iter().filter(|n| tried.contains(&n.sysfs) && !n.health.is_fine()).collect();
    }
    for c in &pending {
        remember_give_up(c);
        log(&format!("iyakki.display_retry connector={} why=\"{why}\" gave up after {} attempts: {}", c.short(), delays.len(), c.health.describe()));
    }
    out
}

#[cfg(target_os = "linux")]
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Where the `iyakki.display_retry` lines go: devctl has no logger of its own, the program passes its one.
static LOGGER: Mutex<Option<fn(&str)>> = Mutex::new(None);

/// Set where the `iyakki.display_retry` lines go (called once at start).
pub fn set_logger(f: fn(&str)) {
    if let Ok(mut l) = LOGGER.lock() {
        *l = Some(f);
    }
}

#[cfg(target_os = "linux")]
fn log_line(l: &str) {
    if let Some(f) = LOGGER.lock().ok().and_then(|g| *g) {
        f(l);
    }
}

/// [`recover`] on `sys_drm` (the real `/sys/class/drm`) in its own thread (one at a time; a second request while one runs is dropped).
/// `changed` runs when at least one connector's list became right: whatever draws the screen paints everything again.
#[cfg(target_os = "linux")]
pub fn recover_in_background(sys_drm: std::path::PathBuf, why: &'static str, force: bool, changed: fn()) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("iyakki-display-retry".into()).spawn(move || {
        let r = recover(&sys_drm, why, force, &RETRY_DELAYS, &mut std::thread::sleep, &mut log_line);
        RUNNING.store(false, Ordering::SeqCst);
        if r.iter().any(|x| x.fixed) {
            changed();
        }
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Without a Linux GPU there is nothing to ask.
#[cfg(not(target_os = "linux"))]
pub fn recover_in_background(_sys_drm: std::path::PathBuf, _why: &'static str, _force: bool, _changed: fn()) {}

/// One line for Settings: how each connected connector looks.
pub fn status_text(sys_drm: &Path) -> String {
    let c = check_all(sys_drm);
    if c.is_empty() {
        return "No connected display was found.".into();
    }
    c.iter().map(|c| format!("{}: {}", c.short(), if c.health.is_fine() { "mode list matches the display".to_string() } else { c.health.describe() })).collect::<Vec<_>>().join("; ")
}

/// "Look again": the status now, and a background retry that ignores earlier give-ups. `changed` runs when a list became right.
pub fn look_again(sys_drm: &Path, changed: fn()) -> String {
    let now = status_text(sys_drm);
    recover_in_background(sys_drm.to_path_buf(), "look again", true, changed);
    now
}

#[cfg(test)]
#[path = "drmmodes_tests.rs"]
mod tests;
