//! The battery indicator in the top bar of a laptop, the popup it opens and the low-battery cards.
//!
//! Only a machine that has a battery shows anything (`iyakki_agent::battery`: a desktop, a virtual machine and a Mac mini show
//! nothing). The icon is drawn here with strokes and fills (no font, no picture): an outline with a nub, a fill in the theme's
//! colours (green, amber under 20 %, red under 10 %), a bolt while charging, a plug when the adapter is in and the battery is
//! full or protecting itself, a question mark when the level is not known. The percentage sits beside it (Settings can hide it).
//! A click or F4 opens the popup: percentage bar, status in plain words, time left or to full, health, adapter, and the reason
//! when the kernel cannot read the battery.
//!
//! [`BatteryBar::poll`] reads `/sys` every 10 s (60 s when idle, once a second while a shutdown countdown runs; the adapter's
//! `online` file every second, so plugging in shows at once) and hands the warnings to the shell. The decisions (what is
//! critical, when a countdown starts, what cancels it) are `iyakki_agent::battery::Alerter`'s, tested there.

use crate::gfx::{rgb, Face, Gfx};
use crate::safeclamp::SafeClamp;
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_agent::battery::{self, Alert, Alerter, BatteryEnv, BatterySettings, IconState, LowBand, Presence, Reading, Tracker};

/// Click regions.
pub const CLOSE: usize = 1_300_011;
pub const CANCEL: usize = 1_300_012;
pub const CARD_OK: usize = 1_300_013;
pub const SETTINGS: usize = 1_300_014;

/// The F-key that opens the popup (the chip's own click is the same event).
pub const KEY: u8 = 4;

/// A chip's place in a bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub rect: R,
    /// The percentage text, when there is room and it is switched on.
    pub text: Option<String>,
    pub text_size: f32,
}

pub struct BatteryBar {
    env: BatteryEnv,
    pub reading: Reading,
    tracker: Tracker,
    alerter: Alerter,
    pub settings: BatterySettings,
    pub open: bool,
    /// The chip last drawn (the popup hangs under it).
    pub anchor: Option<R>,
    /// The "Plug in now" card is showing.
    pub plug_card: bool,
    polled: bool,
    last_full: u64,
    last_ac: u64,
    /// The clock the countdown card reads (seconds from the shell's steady clock).
    now: u64,
}

impl Default for BatteryBar {
    fn default() -> BatteryBar {
        // tests never look at the real machine
        let env = if cfg!(test) { BatteryEnv::at("/nonexistent-iyakki-sys") } else { BatteryEnv::real() };
        BatteryBar::with_env(env)
    }
}

impl BatteryBar {
    pub fn with_env(env: BatteryEnv) -> BatteryBar {
        BatteryBar { env, reading: Reading::default(), tracker: Tracker::new(), alerter: Alerter::new(), settings: BatterySettings::default(), open: false, anchor: None, plug_card: false, polled: false, last_full: 0, last_ac: 0, now: 0 }
    }

    /// Look at another folder than /sys (tests) and read it at the next poll.
    pub fn set_env(&mut self, env: BatteryEnv) {
        self.env = env;
        self.polled = false;
    }

    pub fn shown(&self) -> bool {
        self.reading.shown()
    }

    pub fn counting_down(&self) -> bool {
        self.alerter.counting_down()
    }

    /// Whether Esc has something to dismiss.
    pub fn card_visible(&self) -> bool {
        self.plug_card || self.alerter.counting_down()
    }

    pub fn toggle(&mut self) {
        self.open = self.shown() && !self.open;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// The person pressed Cancel on the countdown card.
    pub fn cancel_countdown(&mut self) {
        self.alerter.cancel_by_person();
    }

    pub fn dismiss_card(&mut self) {
        self.plug_card = false;
    }

    /// Read the machine when it is time (`now`: seconds from a steady clock; `idle`: nobody is looking). Returns what the shell
    /// should tell the person or do. The adapter's file is read every second so a cable shows at once and cancels a countdown.
    pub fn poll(&mut self, now: u64, idle: bool) -> Vec<Alert> {
        self.now = now;
        let every = if self.alerter.counting_down() {
            1
        } else if !self.reading.shown() || idle {
            battery::POLL_IDLE_SECS
        } else {
            battery::POLL_ACTIVE_SECS
        };
        let mut due = !self.polled || now.saturating_sub(self.last_full) >= every;
        if !due && self.reading.shown() && now.saturating_sub(self.last_ac) >= 1 {
            self.last_ac = now;
            due = battery::ac_online(&self.env) != self.reading.ac_online;
        }
        if !due {
            return Vec::new();
        }
        let mut r = battery::read(&self.env);
        self.tracker.apply(&mut r);
        self.polled = true;
        self.last_full = now;
        self.last_ac = now;
        let alerts = self.alerter.step(&r, now, battery::AlertCfg::from(&self.settings));
        if r.presence != Presence::Present || r.status == battery::Status::Charging || r.ac_online == Some(true) {
            self.plug_card = false;
        }
        for a in &alerts {
            match a {
                Alert::PlugInNow => self.plug_card = true,
                Alert::CountdownStart | Alert::Shutdown => self.plug_card = false,
                _ => {}
            }
        }
        if !r.shown() {
            self.open = false;
        }
        self.reading = r;
        alerts
    }

    // ---- the chip

    /// Where the chip goes in a bar: right edge at `right`, row `y` and `box_h` tall, not left of `left_limit`. The percentage is
    /// left out when it does not fit; None when not even the icon fits (or there is no battery).
    pub fn plan(&self, g: &mut Gfx, s: f32, right: f32, left_limit: f32, y: f32, box_h: f32, text_size: f32) -> Option<Plan> {
        if !self.reading.shown() {
            return None;
        }
        let ih = icon_height(box_h);
        let pad = 7.0 * s;
        let icon_w = icon_width(ih);
        let text = (self.settings.show_percent() && self.reading.readable()).then(|| self.reading.percent_text());
        let full = text.as_ref().map(|t| 2.0 * pad + icon_w + 6.0 * s + g.text_width(text_size, t, Face::Bold));
        let plain = 2.0 * pad + icon_w;
        let (w, text) = match full {
            Some(w) if right - w >= left_limit => (w, text),
            _ if right - plain >= left_limit => (plain, None),
            _ => return None,
        };
        Some(Plan { rect: R::new(right - w, y, w, box_h), text, text_size })
    }

    /// Draw a planned chip and register its click (F4). Remembers it as the popup's anchor.
    pub fn draw_chip(&mut self, g: &mut Gfx, p: &Plan, s: f32) {
        let b = p.rect;
        if g.hovered(b) || self.open {
            g.rrect(b.x, b.y, b.w, b.h, 8.0 * s, rgb(28, 36, 66));
        }
        let ih = icon_height(b.h);
        let pad = 7.0 * s;
        draw_icon(g, b.x + pad, b.y + b.h / 2.0, ih, s, &self.reading);
        if let Some(t) = &p.text {
            let ink = match self.reading.low_band() {
                LowBand::Red => ERR,
                LowBand::Amber => crate::fx::AMBER,
                LowBand::Normal => TEXT,
            };
            let tx = b.x + pad + icon_width(ih) + 6.0 * s;
            g.text(tx, b.y + (b.h - g.line_height(p.text_size, Face::Bold)) / 2.0, p.text_size, ink, t, Face::Bold);
        }
        g.hit(b, Event::Key(Key::F(KEY)), &self.reading.summary());
        self.anchor = Some(b);
    }

    // ---- the popup

    /// The popup's panel: under the chip, right-aligned with it, kept on screen.
    pub fn panel(&self, w: u32, h: u32, s: f32, rows: usize) -> R {
        let pw = (380.0 * s).min(w as f32 - 24.0 * s).max(120.0);
        let ph = ((120.0 + 24.0 * rows as f32) * s + 60.0 * s).min(h as f32 - 24.0 * s).max(80.0);
        let top = self.anchor.map_or(56.0 * s, |a| a.y + a.h + 8.0 * s);
        let right = self.anchor.map_or(w as f32 - 12.0 * s, |a| a.x + a.w);
        let x = (right - pw).clamp_safe(12.0 * s, (w as f32 - pw - 12.0 * s).max(12.0 * s));
        let y = top.clamp_safe(8.0 * s, (h as f32 - ph - 8.0 * s).max(8.0 * s));
        R::new(x, y, pw, ph)
    }

    /// Draw the popup. Registers its own click regions and clears the others (a click outside closes it).
    pub fn render(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if !self.open || !self.reading.shown() {
            return;
        }
        g.clear_hits();
        let rows = self.reading.rows();
        let p = self.panel(w, h, s, rows.len() + usize::from(self.reading.presence == Presence::NoDriver));
        crate::fx::modal_panel(g, p, s, crate::fx::CYAN, false);
        g.text(p.x + 20.0 * s, p.y + 14.0 * s, 19.0 * s, TEXT, "Battery", Face::Bold);
        let close = R::new(p.x + p.w - 44.0 * s, p.y + 8.0 * s, 34.0 * s, 34.0 * s);
        let hot = g.hovered(close);
        crate::fx::chrome_button(g, close, s, hot, crate::fx::CYAN);
        let cw = g.text_width(18.0 * s, "x", Face::Bold);
        g.text(close.x + (close.w - cw) / 2.0, close.y + 6.0 * s, 18.0 * s, MUTED, "x", Face::Bold);
        g.hit(close, Event::Tab(CLOSE), "close");
        let mut y = p.y + 54.0 * s;
        if self.reading.readable() {
            let bar = R::new(p.x + 20.0 * s, y, p.w - 40.0 * s, 14.0 * s);
            draw_bar(g, bar, s, &self.reading);
            y += 28.0 * s;
        }
        let size = (13.5 * s).max(12.0);
        let lw = (112.0 * s).min(p.w * 0.36);
        for (k, v) in &rows {
            if y + 20.0 * s > p.y + p.h - 48.0 * s {
                break;
            }
            g.text(p.x + 20.0 * s, y, size, MUTED, k, Face::Sans);
            let lines = g.wrap(size, v, Face::Bold, p.w - 40.0 * s - lw);
            for (i, l) in lines.iter().take(3).enumerate() {
                g.text(p.x + 20.0 * s + lw, y + i as f32 * 18.0 * s, size, TEXT, l, Face::Bold);
            }
            y += 22.0 * s + lines.len().clamp(1, 3).saturating_sub(1) as f32 * 18.0 * s;
        }
        let by = p.y + p.h - 42.0 * s;
        let bw = g.button(p.x + 16.0 * s, by, 13.0 * s, "Battery settings", crate::fx::CYAN, Event::Tab(SETTINGS));
        let hint = g.fit(12.0 * s, "Esc closes", Face::Sans, (p.w - bw - 50.0 * s).max(0.0));
        g.text(p.x + 16.0 * s + bw + 12.0 * s, by + 8.0 * s, 12.0 * s, DIM, &hint, Face::Sans);
    }

    // ---- the cards

    /// The countdown card (centre of the screen) or the "Plug in now" card (top centre), drawn over everything. The countdown
    /// card does not clear the other click regions: a person can still save work with the mouse while it counts.
    pub fn render_cards(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if let Some(left) = self.alerter.countdown_left(self.now) {
            let pw = (480.0 * s).min(w as f32 - 24.0 * s).max(160.0);
            let ph = (170.0 * s).min(h as f32 - 24.0 * s).max(100.0);
            let p = R::new((w as f32 - pw) / 2.0, ((h as f32 - ph) / 2.0).max(8.0 * s), pw, ph);
            crate::fx::modal_panel(g, p, s, ERR, true);
            g.text(p.x + 24.0 * s, p.y + 18.0 * s, 20.0 * s, TEXT, "Battery almost empty", Face::Bold);
            let msg = format!("Shutting down in {left} s to protect your files. Plug in the power adapter to cancel.");
            for (i, l) in g.wrap(14.0 * s, &msg, Face::Sans, p.w - 48.0 * s).iter().take(3).enumerate() {
                g.text(p.x + 24.0 * s, p.y + 54.0 * s + i as f32 * 20.0 * s, 14.0 * s, MUTED, l, Face::Sans);
            }
            g.button(p.x + 24.0 * s, p.y + p.h - 52.0 * s, 15.0 * s, "Cancel shutdown (Esc)", ERR, Event::Tab(CANCEL));
        } else if self.plug_card {
            let pw = (420.0 * s).min(w as f32 - 24.0 * s).max(160.0);
            let ph = (110.0 * s).min(h as f32 - 24.0 * s).max(80.0);
            let p = R::new((w as f32 - pw) / 2.0, 64.0 * s, pw, ph);
            crate::fx::modal_panel(g, p, s, crate::fx::AMBER, false);
            g.text(p.x + 22.0 * s, p.y + 14.0 * s, 19.0 * s, TEXT, "Plug in now", Face::Bold);
            let msg = format!("The battery is at {}. Save your work and plug in the power adapter.", self.reading.percent_text());
            for (i, l) in g.wrap(13.5 * s, &msg, Face::Sans, p.w - 44.0 * s).iter().take(2).enumerate() {
                g.text(p.x + 22.0 * s, p.y + 44.0 * s + i as f32 * 19.0 * s, 13.5 * s, MUTED, l, Face::Sans);
            }
            g.button(p.x + p.w - 90.0 * s, p.y + p.h - 40.0 * s, 13.0 * s, "OK", crate::fx::AMBER, Event::Tab(CARD_OK));
        }
    }
}

/// The toast for a warning level.
pub fn toast_text(level: u8, r: &Reading) -> String {
    let pct = r.percent_whole().map_or_else(String::new, |p| format!(" ({p}%)"));
    match level {
        10 => format!("Battery low{pct}. Plug in the power adapter soon."),
        _ => format!("Battery at {level}%{pct}. Plug in when you can."),
    }
}

// ---- drawing the icon

/// Height of the icon's body for a bar of `box_h`.
pub fn icon_height(box_h: f32) -> f32 {
    (box_h * 0.42).max(8.0)
}

/// Width of the icon with its nub.
pub fn icon_width(ih: f32) -> f32 {
    ih * 1.9 + ih * 0.16
}

/// The fill colour: green, amber under 20 %, red under 10 %.
pub fn fill_color(r: &Reading) -> [u8; 4] {
    match r.low_band() {
        LowBand::Red => ERR,
        LowBand::Amber => crate::fx::AMBER,
        LowBand::Normal => OK,
    }
}

/// Draw the battery icon with its left edge at `x`, centred on `cy`, `ih` tall. Returns the body's box (where the fill is).
pub fn draw_icon(g: &mut Gfx, x: f32, cy: f32, ih: f32, s: f32, r: &Reading) -> R {
    let bw = ih * 1.9;
    let y = cy - ih / 2.0;
    let line = (1.5 * s).max(1.2);
    let unknown = r.icon_state() == IconState::Unknown;
    let edge = if unknown { DIM } else { MUTED };
    g.rrect_outline(x + line / 2.0, y + line / 2.0, bw - line, ih - line, ih * 0.2, line, edge);
    // the nub
    g.rrect(x + bw + ih * 0.03, y + ih * 0.3, ih * 0.13, ih * 0.4, ih * 0.05, edge);
    // the fill
    let inset = line + (1.0 * s).max(0.8);
    let inner_w = bw - 2.0 * inset;
    let frac = r.fill_fraction();
    if r.readable() && frac > 0.0 {
        let fw = (inner_w * frac).max(2.0 * s).min(inner_w).max(0.0);
        g.rrect(x + inset, y + inset, fw, (ih - 2.0 * inset).max(1.0), (ih * 0.12).max(0.5), fill_color(r));
    }
    let (mx, my) = (x + bw / 2.0, cy);
    match r.icon_state() {
        IconState::Charging => draw_bolt(g, mx, my, ih),
        IconState::Plugged => draw_plug(g, mx, my, ih),
        IconState::Unknown => {
            let size = ih * 0.95;
            let tw = g.text_width(size, "?", Face::Bold);
            g.text(mx - tw / 2.0, my - g.line_height(size, Face::Bold) / 2.0, size, TEXT, "?", Face::Bold);
        }
        IconState::Discharging => {}
    }
    R::new(x, y, bw, ih)
}

const SHADOW: [u8; 4] = [8, 10, 20, 230];
const SYMBOL: [u8; 4] = [255, 255, 255, 255];

fn bolt_points(cx: f32, cy: f32, ih: f32, grow: f32) -> [(f32, f32); 6] {
    let (hw, hh) = (ih * 0.2 + grow, ih * 0.5 + grow);
    [(cx + hw * 0.15, cy - hh), (cx - hw, cy + hh * 0.1), (cx - hw * 0.05, cy + hh * 0.1), (cx - hw * 0.2, cy + hh), (cx + hw, cy - hh * 0.15), (cx + hw * 0.05, cy - hh * 0.15)]
}

/// A lightning bolt, white with a dark edge so it reads on the fill and on the empty part.
fn draw_bolt(g: &mut Gfx, cx: f32, cy: f32, ih: f32) {
    g.polygon(&bolt_points(cx, cy, ih, ih * 0.06), SHADOW);
    g.polygon(&bolt_points(cx, cy, ih, 0.0), SYMBOL);
}

/// A plug: a body with two prongs on top.
fn draw_plug(g: &mut Gfx, cx: f32, cy: f32, ih: f32) {
    let (bw, bh) = (ih * 0.56, ih * 0.38);
    let by = cy - bh / 2.0 + ih * 0.06;
    let w = (ih * 0.1).max(1.0);
    for (grow, c) in [(w * 0.7, SHADOW), (0.0, SYMBOL)] {
        g.rrect(cx - bw / 2.0 - grow, by - grow, bw + 2.0 * grow, bh + 2.0 * grow, ih * 0.08, c);
        for dx in [-0.14, 0.14] {
            g.line(cx + dx * ih, by - ih * 0.2, cx + dx * ih, by, w + grow * 2.0, c);
        }
        g.line(cx, by + bh, cx, by + bh + ih * 0.12, w + grow * 2.0, c);
    }
}

/// The popup's percentage bar.
fn draw_bar(g: &mut Gfx, r: R, s: f32, rd: &Reading) {
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(26, 34, 62));
    let f = rd.fill_fraction();
    if f > 0.0 {
        g.rrect(r.x, r.y, (r.w * f).max(r.h).min(r.w), r.h, r.h / 2.0, fill_color(rd));
    }
    let t = rd.percent_text();
    let size = 11.0 * s;
    let tw = g.text_width(size, &t, Face::Bold);
    let ink = if f >= 0.55 { rgb(10, 14, 28) } else { TEXT };
    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - g.line_height(size, Face::Bold)) / 2.0, size, ink, &t, Face::Bold);
}
