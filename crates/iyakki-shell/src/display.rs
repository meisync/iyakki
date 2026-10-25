//! Which part of the framebuffer the monitor really shows.
//!
//! On DRM fbdev emulation the buffer can stay at the firmware's size (a MacBookPro14,1: 2880x1800 from EFI) while the display
//! engine scans out the panel's native mode (2560x1600) from the top-left corner of it. `FBIOGET_VSCREENINFO` may still say
//! 2880x1800, so a shell laid out for the buffer has its right and bottom parts (Back, the clock) off screen. The active
//! mode is therefore read from `/sys/class/drm`, and the shell lays itself out for that visible rectangle only.

use std::path::Path;

pub mod kms;
pub mod outputs;

/// One display connector as sysfs describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector {
    /// "eDP-1", "HDMI-A-1" (the part of the sysfs directory name after `cardN-`).
    pub name: String,
    pub connected: bool,
    /// `enabled` file when readable: "enabled" is true, "disabled" false.
    pub enabled: Option<bool>,
    /// The `modes` file in order, the first being the preferred one. Zero-size and unparsable lines are kept out.
    pub modes: Vec<(u32, u32)>,
}

/// The size to lay the shell out for, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visible {
    pub width: u32,
    pub height: u32,
    /// "eDP-1 preferred mode", or what made the buffer size the answer.
    pub source: String,
}

impl Visible {
    /// True when the monitor shows less than the whole buffer.
    pub fn smaller_than(&self, buffer: (u32, u32)) -> bool {
        (self.width, self.height) != buffer
    }
}

/// Parse "2560x1600", "2560x1600p-60" or "U:2880x1800p-0" (the first `WxH` found).
pub fn parse_mode(line: &str) -> Option<(u32, u32)> {
    let s = line.trim();
    let s = s.split_once(':').map_or(s, |(_, rest)| rest);
    let (w, rest) = s.split_once('x')?;
    let w: u32 = w.trim().parse().ok()?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let h: u32 = digits.parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// Read every `<drm>/card*-*` connector directory. Unreadable things are simply left out; never fails.
pub fn read_connectors(drm: &Path) -> Vec<Connector> {
    let Ok(rd) = std::fs::read_dir(drm) else { return Vec::new() };
    let mut dirs: Vec<_> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("card") && n.contains('-')).collect();
    dirs.sort();
    let mut out = Vec::new();
    for d in dirs {
        let p = drm.join(&d);
        let read = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|v| v.trim().to_string());
        let Some(status) = read("status") else { continue };
        let name = d.split_once('-').map(|(_, n)| n.to_string()).unwrap_or_else(|| d.clone());
        let modes = read("modes").map(|m| m.lines().filter_map(parse_mode).collect()).unwrap_or_default();
        let enabled = read("enabled").map(|e| e == "enabled");
        out.push(Connector { name, connected: status == "connected", enabled, modes });
    }
    out
}

/// A built-in panel: the one that is "the" screen when several are connected.
fn is_internal(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("edp") || n.contains("lvds") || n.contains("dsi")
}

/// The size the monitor shows of a `buffer`, given the size `fb_var` the framebuffer driver reports (`xres`, `yres`) and the
/// connectors.
///
/// Rules: the base is `fb_var` when it is non-zero and fits the buffer, else the buffer. Among the connected connectors (those
/// reported enabled when any is, since a disabled one scans nothing) with a usable preferred mode, the primary is the built-in
/// panel (eDP, LVDS, DSI, first by name), else the one with the smallest mode, because the same buffer is scanned out on
/// every connector and the smallest shows the least. Its preferred mode is the visible size when it fits the base in both
/// dimensions; a larger mode (or a rotated one that does not fit) cannot be a part of the buffer, so the base stays. Nothing
/// readable gives the base. Sizes only: a rotated panel is not rotated.
pub fn visible_size(buffer: (u32, u32), fb_var: (u32, u32), connectors: &[Connector]) -> Visible {
    let fits = |m: (u32, u32), of: (u32, u32)| m.0 > 0 && m.1 > 0 && m.0 <= of.0 && m.1 <= of.1;
    let (base, base_src) = if fits(fb_var, buffer) && fb_var != buffer { (fb_var, "framebuffer reports it") } else { (buffer, "framebuffer size") };
    let connected: Vec<&Connector> = connectors.iter().filter(|c| c.connected && c.modes.iter().any(|m| m.0 > 0 && m.1 > 0)).collect();
    let on: Vec<&Connector> = connected.iter().copied().filter(|c| c.enabled == Some(true)).collect();
    let pool = if on.is_empty() { connected } else { on };
    let preferred = |c: &Connector| c.modes.iter().copied().find(|m| m.0 > 0 && m.1 > 0);
    let primary = pool.iter().filter(|c| is_internal(&c.name)).min_by(|a, b| a.name.cmp(&b.name)).or_else(|| pool.iter().min_by_key(|c| preferred(c).map_or(u64::MAX, |m| m.0 as u64 * m.1 as u64)));
    if let Some(c) = primary {
        if let Some(m) = preferred(c) {
            if fits(m, base) {
                let source = if m == buffer { format!("{} preferred mode, same as the buffer", c.name) } else { format!("{} preferred mode", c.name) };
                return Visible { width: m.0, height: m.1, source };
            }
            return Visible { width: base.0, height: base.1, source: format!("{} preferred mode {}x{} does not fit the buffer; {base_src}", c.name, m.0, m.1) };
        }
    }
    Visible { width: base.0, height: base.1, source: format!("{base_src}; no connected display mode readable") }
}

/// [`visible_size`] with the real sysfs.
pub fn visible_now(buffer: (u32, u32), fb_var: (u32, u32)) -> Visible {
    visible_size(buffer, fb_var, &read_connectors(Path::new("/sys/class/drm")))
}

/// What identifies the displays right now (connected connectors and their preferred mode), to notice a hot-plug or a mode change.
pub fn signature(drm: &Path) -> String {
    read_connectors(drm).iter().filter(|c| c.connected).map(|c| format!("{}:{}", c.name, c.modes.first().map_or("?".to_string(), |m| format!("{}x{}", m.0, m.1)))).collect::<Vec<_>>().join(",")
}

/// Black out everything of a `pitch`-byte-per-row buffer of `rows` rows outside the top-left `w`x`h` pixels (`bytes` per pixel),
/// so a later mode change shows nothing odd. `mem` may be shorter than `pitch * rows`.
pub fn clear_outside(mem: &mut [u8], pitch: usize, rows: usize, w: usize, h: usize, bytes: usize) {
    let edge = (w * bytes).min(pitch);
    for y in 0..rows.min(mem.len().div_ceil(pitch.max(1))) {
        let start = y * pitch;
        let end = (start + pitch).min(mem.len());
        let from = if y < h { start + edge } else { start };
        if from < end {
            mem[from..end].fill(0);
        }
    }
}

#[cfg(test)]
#[path = "display_tests.rs"]
mod tests;
