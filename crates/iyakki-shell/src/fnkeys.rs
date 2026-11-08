//! Media and Fn keys: the key map, step curves, backlight writers, the volume change and the on-screen display (OSD).
//!
//! The input thread turns a recognised evdev key into `Event::Fn(key, down)`; the shell takes it before any app sees it (so a
//! volume key never reaches the web page as a letter) and acts through the functions here. Everything that touches the
//! machine hangs off a `sys` root (`/sys`), so the tests use a fake directory. The shell runs as root on the IYAKKI image, so
//! the backlight files are written directly, the way Settings -> Display already does (`iyakki_devctl::set_brightness`).

use crate::safeclamp::SafeClamp;
use std::path::{Path, PathBuf};

use iyakki_agent::audioout::SoundSettings;

use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{R, TEXT};

// ------------------------------------------------------------------------------------------------ the key map

pub use iyakki_uikit::keys::FnKey;

/// The saved scroll preferences as the toolkit's scroll engine wants them (the engine lives in `iyakki-uikit` and cannot name the
/// agent's type, so this is a function and not a `From` impl).
pub fn scroll_config(p: iyakki_agent::keyboard::ScrollPrefs) -> crate::scrolleng::ScrollConfig {
    crate::scrolleng::ScrollConfig { speed_pct: p.speed_pct, wheel_natural: p.wheel_natural, pad_natural: p.pad_natural, smooth: p.smooth, momentum: p.momentum, wheel_lines: p.wheel_lines }
}

pub struct KeyInfo {
    pub code: u16,
    pub name: &'static str,
    pub key: FnKey,
    /// What the shell does, in the words of the Settings table.
    pub action: &'static str,
}

const fn k(code: u16, name: &'static str, key: FnKey, action: &'static str) -> KeyInfo {
    KeyInfo { code, name, key, action }
}

/// Every evdev key code the shell reacts to, with what it does. The Settings table and the docs come from this list.
pub const KEYS: &[KeyInfo] = &[
    k(113, "KEY_MUTE", FnKey::VolumeMute, "Mute or unmute (the level is kept)"),
    k(114, "KEY_VOLUMEDOWN", FnKey::VolumeDown, "Volume down 5 %"),
    k(115, "KEY_VOLUMEUP", FnKey::VolumeUp, "Volume up 5 %"),
    k(248, "KEY_MICMUTE", FnKey::MicMute, "Microphone mute (planned; says so)"),
    k(224, "KEY_BRIGHTNESSDOWN", FnKey::BrightnessDown, "Screen dimmer"),
    k(225, "KEY_BRIGHTNESSUP", FnKey::BrightnessUp, "Screen brighter"),
    k(244, "KEY_BRIGHTNESS_AUTO", FnKey::BrightnessAuto, "Automatic brightness (not supported; says so)"),
    k(229, "KEY_KBDILLUMDOWN", FnKey::KbdLightDown, "Keyboard light dimmer"),
    k(230, "KEY_KBDILLUMUP", FnKey::KbdLightUp, "Keyboard light brighter"),
    k(228, "KEY_KBDILLUMTOGGLE", FnKey::KbdLightToggle, "Keyboard light on or off"),
    k(164, "KEY_PLAYPAUSE", FnKey::PlayPause, "Play or pause (radio, media player)"),
    k(200, "KEY_PLAYCD", FnKey::PlayPause, "Play or pause (radio, media player)"),
    k(201, "KEY_PAUSECD", FnKey::PlayPause, "Play or pause (radio, media player)"),
    k(207, "KEY_PLAY", FnKey::PlayPause, "Play or pause (radio, media player)"),
    k(163, "KEY_NEXTSONG", FnKey::Next, "Next station or skip forward"),
    k(165, "KEY_PREVIOUSSONG", FnKey::Prev, "Previous station or skip back"),
    k(166, "KEY_STOPCD", FnKey::Stop, "Stop the radio or the player"),
    k(142, "KEY_SLEEP", FnKey::Sleep, "Opens the power card (Sleep is planned)"),
    k(205, "KEY_SUSPEND", FnKey::Sleep, "Opens the power card (Sleep is planned)"),
    k(116, "KEY_POWER", FnKey::Power, "Opens the power card: nothing happens until you choose"),
    k(247, "KEY_RFKILL", FnKey::RfKill, "Airplane mode: all radios off or on"),
    k(238, "KEY_WLAN", FnKey::Wlan, "WiFi off or on"),
    k(237, "KEY_BLUETOOTH", FnKey::Bluetooth, "Bluetooth off or on"),
    k(227, "KEY_SWITCHVIDEOMODE", FnKey::SwitchVideo, "Notice only (one display mode is used)"),
    k(210, "KEY_PRINT", FnKey::Print, "Screenshot"),
    k(140, "KEY_CALC", FnKey::Calc, "Notice only (no calculator app)"),
    k(172, "KEY_HOMEPAGE", FnKey::Homepage, "Opens the browser (from the launcher)"),
];

pub fn fn_key(code: u16) -> Option<FnKey> {
    KEYS.iter().find(|i| i.code == code).map(|i| i.key)
}

pub fn info_for(code: u16) -> Option<&'static KeyInfo> {
    KEYS.iter().find(|i| i.code == code)
}

/// Keys seen that are not letters or the usual keys, newest last (for Settings -> Keyboard: "find your key codes").
static SEEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
const SEEN_MAX: usize = 8;

pub fn note_seen(code: u16) {
    if let Ok(mut v) = SEEN.lock() {
        v.retain(|c| *c != code);
        v.push(code);
        if v.len() > SEEN_MAX {
            v.remove(0);
        }
    }
}

pub fn seen() -> Vec<u16> {
    SEEN.lock().map(|v| v.clone()).unwrap_or_default()
}

/// "225 KEY_BRIGHTNESSUP: Screen brighter" or "249 (not used by IYAKKI)".
pub fn describe_code(code: u16) -> String {
    match info_for(code) {
        Some(i) => format!("{code} {}: {}", i.name, i.action),
        None => format!("{code} (a key IYAKKI does not use)"),
    }
}

// ------------------------------------------------------------------------------------------------ volume

pub const VOLUME_STEP: u32 = 5;
/// The level a muted output of volume 0 comes back at.
pub const UNMUTE_LEVEL: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolOp {
    Up,
    Down,
    Mute,
}

/// Change IYAKKI's volume the way the keys do: steps of 5 % on the multiples of 5, a step also unmutes, Mute keeps the level.
pub fn volume_apply(s: &mut SoundSettings, op: VolOp) {
    let v = s.volume_pct.min(100);
    match op {
        VolOp::Up => {
            s.muted = false;
            s.volume_pct = (v / VOLUME_STEP * VOLUME_STEP + VOLUME_STEP).min(100);
        }
        VolOp::Down => {
            s.muted = false;
            s.volume_pct = if v % VOLUME_STEP == 0 { v.saturating_sub(VOLUME_STEP) } else { v / VOLUME_STEP * VOLUME_STEP };
        }
        VolOp::Mute => {
            if s.muted {
                s.muted = false;
                if v == 0 {
                    s.volume_pct = UNMUTE_LEVEL;
                }
            } else {
                s.muted = true;
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ backlights

/// A backlight or keyboard LED: `<dir>/brightness` and `<dir>/max_brightness`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Light {
    pub name: String,
    pub dir: PathBuf,
    pub value: u32,
    pub max: u32,
}

impl Light {
    pub fn fraction(&self) -> f32 {
        if self.max == 0 { 0.0 } else { (self.value as f32 / self.max as f32).clamp(0.0, 1.0) }
    }
    pub fn percent(&self) -> u32 {
        (self.fraction() * 100.0).round() as u32
    }
}

fn read_u32(p: &Path) -> Option<u32> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn lights_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<Light> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Light> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep(&name) {
                return None;
            }
            let d = e.path();
            let max = read_u32(&d.join("max_brightness"))?;
            let value = read_u32(&d.join("brightness"))?;
            (max > 0).then_some(Light { name, dir: d, value, max })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Lower is better: the panel's own driver before the generic ACPI one.
fn panel_rank(name: &str) -> u32 {
    const ORDER: [&str; 7] = ["intel_backlight", "amdgpu_bl", "radeon_bl", "nvidia", "nv_backlight", "apple-panel-bl", "gmux_backlight"];
    ORDER.iter().position(|p| name.starts_with(p)).map(|i| i as u32).unwrap_or(if name.starts_with("acpi_video") { 100 } else { 50 })
}

/// Every backlight of the machine (the Touch Bar's included), best panel first.
pub fn all_backlights(sys: &Path) -> Vec<Light> {
    let mut v = lights_in(&sys.join("class/backlight"), |_| true);
    v.sort_by_key(|l| (panel_rank(&l.name), l.name.clone()));
    v
}

/// The backlights that dim the screen: the Touch Bar has one of its own (`appletb_backlight`) that is not the display.
pub fn panel_backlights(sys: &Path) -> Vec<Light> {
    all_backlights(sys).into_iter().filter(|l| !l.name.starts_with("appletb")).collect()
}

/// Keyboard backlights (`smc::kbd_backlight` on Macs, `tpacpi::kbd_backlight`, `asus::kbd_backlight`, ...).
pub fn kbd_backlights(sys: &Path) -> Vec<Light> {
    lights_in(&sys.join("class/leds"), |n| n.contains("kbd_backlight") || n.contains("kbd_illum"))
}

pub const STEPS: usize = 16;

/// About 16 steps that look even to the eye (a square law), never 0: the lowest is about 2 % of the maximum, at least 1.
/// A panel with a small range has fewer (equal values are dropped).
pub fn brightness_levels(max: u32) -> Vec<u32> {
    if max == 0 {
        return Vec::new();
    }
    let mut v: Vec<u32> = Vec::new();
    for i in 0..STEPS {
        let t = i as f64 / (STEPS - 1) as f64;
        let val = ((max as f64 * (0.02 + 0.98 * t * t)).round() as u32).clamp_safe(1, max);
        if v.last() != Some(&val) {
            v.push(val);
        }
    }
    v
}

/// Keyboard light steps: off, then up to eight even steps.
pub fn kbd_levels(max: u32) -> Vec<u32> {
    if max == 0 {
        return Vec::new();
    }
    let mut v: Vec<u32> = (0..=max.min(8)).map(|i| if max <= 8 { i } else { i * max / 8 }).collect();
    v.dedup();
    v
}

/// The next level above (or below) `cur`; the end of the range stays where it is.
pub fn step_level(levels: &[u32], cur: u32, up: bool) -> u32 {
    if up {
        levels.iter().copied().find(|l| *l > cur).or(levels.last().copied()).unwrap_or(cur)
    } else {
        levels.iter().copied().rev().find(|l| *l < cur).or(levels.first().copied()).unwrap_or(cur)
    }
}

/// The result of a key: what the display shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub name: String,
    pub value: u32,
    pub max: u32,
    pub fraction: f32,
    /// Nothing moved: already at the end of the range.
    pub at_end: bool,
}

pub const NO_PANEL: &str = "No brightness control on this display";
pub const NO_KBD: &str = "No keyboard backlight on this computer";

fn write_light(l: &Light, v: u32, what: &str) -> Result<(), String> {
    std::fs::write(l.dir.join("brightness"), v.to_string()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Not allowed to change the {what} (needs root)")
        } else {
            format!("Could not change the {what}: {e}")
        }
    })
}

fn change_of(l: &Light, before: u32, now: u32) -> Change {
    Change { name: l.name.clone(), value: now, max: l.max, fraction: now as f32 / l.max.max(1) as f32, at_end: before == now }
}

/// Screen brightness one step up or down on the best panel backlight.
pub fn adjust_panel(sys: &Path, up: bool) -> Result<Change, String> {
    let l = panel_backlights(sys).into_iter().next().ok_or_else(|| NO_PANEL.to_string())?;
    let now = step_level(&brightness_levels(l.max), l.value, up);
    if now != l.value {
        write_light(&l, now, "brightness")?;
    }
    Ok(change_of(&l, l.value, now))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KbdOp {
    Up,
    Down,
    Toggle,
}

/// Keyboard backlight: steps include off; Toggle switches off and back to the level it had (`last` remembers it).
pub fn adjust_kbd(sys: &Path, op: KbdOp, last: &mut u32) -> Result<Change, String> {
    let l = kbd_backlights(sys).into_iter().next().ok_or_else(|| NO_KBD.to_string())?;
    let now = match op {
        KbdOp::Up => step_level(&kbd_levels(l.max), l.value, true),
        KbdOp::Down => step_level(&kbd_levels(l.max), l.value, false),
        KbdOp::Toggle if l.value > 0 => {
            *last = l.value;
            0
        }
        KbdOp::Toggle => if *last == 0 { (l.max / 2).max(1) } else { (*last).min(l.max) },
    };
    if now != l.value {
        write_light(&l, now, "keyboard backlight")?;
    }
    Ok(change_of(&l, l.value, now))
}

/// A test of a control from Settings: one step up and back (so the person sees it move).
pub fn test_panel(sys: &Path) -> Result<String, String> {
    let l = panel_backlights(sys).into_iter().next().ok_or_else(|| NO_PANEL.to_string())?;
    let levels = brightness_levels(l.max);
    let other = if step_level(&levels, l.value, false) != l.value { step_level(&levels, l.value, false) } else { step_level(&levels, l.value, true) };
    write_light(&l, other, "brightness")?;
    std::thread::sleep(std::time::Duration::from_millis(350));
    write_light(&l, l.value, "brightness")?;
    Ok(format!("{} dimmed for a moment and restored", l.name))
}

// ------------------------------------------------------------------------------------------------ radios

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Radio {
    Wifi,
    Bluetooth,
    All,
}

fn roots(sys: &Path) -> iyakki_devctl::Roots {
    iyakki_devctl::Roots { sys: sys.to_path_buf(), proc_: PathBuf::from("/proc") }
}

fn of_kind(sys: &Path, r: Radio) -> Vec<iyakki_devctl::Rfkill> {
    iyakki_devctl::rfkill_list(&roots(sys)).into_iter().filter(|x| match r {
        Radio::Wifi => x.kind == "wlan",
        Radio::Bluetooth => x.kind == "bluetooth",
        Radio::All => true,
    }).collect()
}

/// (id, soft blocked, hard blocked) of the radios, to see whether the kernel handled the key by itself.
pub fn radio_snapshot(sys: &Path) -> Vec<(String, bool, bool)> {
    of_kind(sys, Radio::All).into_iter().map(|x| (x.id, x.soft, x.hard)).collect()
}

fn radio_name(r: Radio) -> &'static str {
    match r {
        Radio::Wifi => "WiFi",
        Radio::Bluetooth => "Bluetooth",
        Radio::All => "Airplane mode",
    }
}

/// What the state is now, as the notice says it ("WiFi off", "Airplane mode on").
pub fn radio_notice(sys: &Path, r: Radio) -> String {
    let list = of_kind(sys, r);
    if list.is_empty() {
        return match r {
            Radio::All => "No radios to switch".into(),
            _ => format!("No {} adapter", radio_name(r)),
        };
    }
    let off = list.iter().all(|x| x.soft || x.hard);
    match r {
        Radio::All => format!("Airplane mode {}", if off { "on" } else { "off" }),
        _ => format!("{} {}", radio_name(r), if off { "off" } else { "on" }),
    }
}

/// Switch the radios: off when any is on, else on. A hardware switch cannot be undone from software and is said so.
pub fn toggle_radio(sys: &Path, r: Radio) -> Result<String, String> {
    let list = of_kind(sys, r);
    if list.is_empty() {
        return Ok(radio_notice(sys, r));
    }
    let block = list.iter().any(|x| !x.soft && !x.hard);
    for x in &list {
        if x.hard && !block {
            continue;
        }
        iyakki_devctl::rfkill_set(&roots(sys), &x.id, block)?;
    }
    if !block && list.iter().any(|x| x.hard) {
        return Ok(format!("{} is off by a hardware switch", radio_name(r)));
    }
    Ok(radio_notice(sys, r))
}

// ------------------------------------------------------------------------------------------------ on-screen display

/// How long the display stays, in heartbeats (~30 per second).
pub const OSD_TICKS: u64 = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Brightness,
    KbdLight,
    Notice,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Osd {
    pub kind: OsdKind,
    pub text: String,
    /// The bar (None: a notice with text only).
    pub fraction: Option<f32>,
    pub muted: bool,
    pub until: u64,
}

impl Osd {
    pub fn volume(s: &SoundSettings, until: u64) -> Osd {
        let text = if s.muted { "Muted".to_string() } else { format!("{}%", s.volume_pct) };
        Osd { kind: OsdKind::Volume, text, fraction: Some(s.volume_pct.min(100) as f32 / 100.0), muted: s.muted, until }
    }
    pub fn level(kind: OsdKind, c: &Change, until: u64) -> Osd {
        Osd { kind, text: format!("{}%", (c.fraction * 100.0).round() as u32), fraction: Some(c.fraction), muted: false, until }
    }
    pub fn notice(text: impl Into<String>, until: u64) -> Osd {
        Osd { kind: OsdKind::Notice, text: text.into(), fraction: None, muted: false, until }
    }
}

/// Where the display sits: bottom centre, above the toast line.
pub fn osd_rect(w: f32, h: f32, s: f32, text_w: f32, bar: bool) -> R {
    let bw = if bar { 330.0 * s } else { (text_w + 48.0 * s).max(200.0 * s) };
    let bh = 58.0 * s;
    let bw = bw.min(w - 24.0 * s);
    R::new((w - bw) / 2.0, h - 160.0 * s, bw, bh)
}

/// Draw the display. It registers no click region: it never takes focus or input.
pub fn draw_osd(g: &mut Gfx, w: f32, h: f32, s: f32, o: &Osd) {
    let accent = match o.kind {
        OsdKind::Volume => rgb(120, 170, 255),
        OsdKind::Brightness => rgb(255, 214, 110),
        OsdKind::KbdLight => rgb(190, 150, 255),
        OsdKind::Notice => rgb(110, 224, 160),
    };
    let tw = g.text_width(17.0 * s, &o.text, Face::Bold);
    let r = osd_rect(w, h, s, tw, o.fraction.is_some());
    g.rrect_glow(r.x, r.y, r.w, r.h, r.h / 2.0, 6.0 * s, [accent[0], accent[1], accent[2], 60]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(14, 20, 38));
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, accent);
    let Some(f) = o.fraction else {
        let t = g.fit(17.0 * s, &o.text, Face::Bold, r.w - 36.0 * s);
        let t_w = g.text_width(17.0 * s, &t, Face::Bold);
        g.text(r.x + (r.w - t_w) / 2.0, r.y + r.h / 2.0 - 11.0 * s, 17.0 * s, TEXT, &t, Face::Bold);
        return;
    };
    let (ix, iy) = (r.x + 32.0 * s, r.y + r.h / 2.0);
    let ic = if o.muted { rgb(150, 150, 170) } else { accent };
    match o.kind {
        OsdKind::Volume => {
            g.icon(Icon::Speaker, ix, iy, 26.0 * s, ic);
            if o.muted {
                g.line(ix - 13.0 * s, iy + 13.0 * s, ix + 13.0 * s, iy - 13.0 * s, 2.5 * s, rgb(255, 120, 120));
            }
        }
        OsdKind::Brightness => draw_sun(g, ix, iy, 11.0 * s, s, ic),
        _ => {
            // a keyboard: a body with a row of keys
            g.rrect_outline(ix - 14.0 * s, iy - 8.0 * s, 28.0 * s, 16.0 * s, 3.0 * s, 1.6 * s, ic);
            for i in 0..4 {
                g.rect(ix - 10.0 * s + i as f32 * 6.0 * s, iy - 3.0 * s, 4.0 * s, 3.0 * s, ic);
            }
            g.rect(ix - 8.0 * s, iy + 2.0 * s, 16.0 * s, 3.0 * s, ic);
        }
    }
    let (bx, bw) = (r.x + 64.0 * s, r.w - 64.0 * s - 78.0 * s);
    let bar_h = 8.0 * s;
    g.rrect(bx, iy - bar_h / 2.0, bw, bar_h, bar_h / 2.0, rgb(40, 48, 74));
    let fill = if o.muted { 0.0 } else { f.clamp(0.0, 1.0) };
    if fill > 0.0 {
        g.rrect(bx, iy - bar_h / 2.0, (bw * fill).max(bar_h), bar_h, bar_h / 2.0, accent);
    }
    g.text(r.x + r.w - 68.0 * s, iy - 11.0 * s, 17.0 * s, if o.muted { rgb(255, 120, 120) } else { TEXT }, &o.text, Face::Bold);
}

fn draw_sun(g: &mut Gfx, cx: f32, cy: f32, r: f32, s: f32, c: [u8; 4]) {
    g.circle(cx, cy, r * 0.55, c);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::FRAC_PI_4;
        g.line(cx + a.cos() * r * 0.8, cy + a.sin() * r * 0.8, cx + a.cos() * r * 1.15, cy + a.sin() * r * 1.15, 1.8 * s, c);
    }
}

// ------------------------------------------------------------------------------------------------ the shell's state

/// A WiFi, Bluetooth or airplane key waits a moment: when the kernel switched the radio by itself (rfkill-input) the shell only
/// reports it; when nothing changed, the shell switches.
#[derive(Clone, Debug)]
pub struct PendingRadio {
    pub radio: Radio,
    pub at: u64,
    pub before: Vec<(String, bool, bool)>,
}

pub struct FnState {
    pub sys: PathBuf,
    /// Tests say whether a sound output exists; None asks the sound selector.
    pub sound_probe: Option<bool>,
    pub osd: Option<Osd>,
    /// The step key held down: (key, tick of the press, tick of the last step).
    pub held: Option<(FnKey, u64, u64)>,
    pub last_press: Option<(FnKey, u64)>,
    pub radio: Option<PendingRadio>,
    pub kbd_last: u32,
    pub power_card: bool,
    /// Tick of the last time the saved keyboard settings were written to the kernel.
    pub synced: Option<u64>,
}

impl Default for FnState {
    fn default() -> FnState {
        FnState { sys: PathBuf::from("/sys"), sound_probe: None, osd: None, held: None, last_press: None, radio: None, kbd_last: 0, power_card: false, synced: None }
    }
}

/// Is there a sound output to change the volume of? (Not when the sound selector looked and found none.)
pub fn sound_present() -> bool {
    let g = iyakki_agent::audioout::global();
    if !g.is_active() {
        return true;
    }
    let st = g.status();
    st.choice.output.is_some() || st.sound_server
}

#[cfg(test)]
#[path = "fnkeys_tests.rs"]
mod tests;
