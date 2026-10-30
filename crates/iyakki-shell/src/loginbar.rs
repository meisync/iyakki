//! The slim top bar of the login and first-run setup screens (local console only): host name, network, battery, the clock and a
//! Power button that opens a Power off / Restart / Cancel card. The chips are read-only (nobody is signed in, so nothing leads
//! into Settings). The network and battery text come from the same reading the Devices popover uses (`devpop.rs`).
//!
//! The pieces that decide things are plain functions ([`plan`], [`network_chip`], [`battery_chip`], [`card_rects`]) so tests can
//! check the rectangles without drawing.

use crate::devpop::{Level, Snapshot};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// The key that opens the power card on the login screen (the Power button does the same).
pub const POWER_KEY: char = 'p';
/// What the card says when the host cannot power off (windowed development mode, tests).
pub const CANNOT: &str = "This system cannot power off from here";

/// The bar's own scale: never smaller than 0.85, so the Power button stays a target a finger or a pointer can hit.
fn bs(s: f32) -> f32 {
    s.max(0.85)
}

/// Height of the bar; the login card starts below it.
pub fn bar_h(s: f32) -> f32 {
    38.0 * bs(s)
}

/// State kept by the shell for the login screens.
#[derive(Default)]
pub struct LoginBar {
    /// The session came through the browser (remote desktop): no bar, no power.
    pub remote: bool,
    /// The power card is open; the highlighted choice (0 Power off, 1 Restart, 2 Sleep when offered, then Cancel).
    pub card: Option<usize>,
    /// Where the pieces were drawn last frame (tests and tools).
    pub last: Vec<(Chip, R)>,
    /// The Power button's region from the last frame.
    pub power_rect: Option<R>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Host,
    Network,
    Battery,
    Date,
    Time,
    Power,
}

/// What the bar shows.
pub struct Bar {
    pub host: String,
    pub net: (String, bool),
    pub battery: Option<(String, bool)>,
    pub time: String,
    pub date: String,
}

/// The network chip: "Wi-Fi: Name", "Ethernet connected" or "no network", and whether something is connected.
pub fn network_chip(snap: Option<&Snapshot>) -> (String, bool) {
    let Some(snap) = snap else { return ("checking network".into(), false) };
    let mut parts = Vec::new();
    for it in snap.items.iter().filter(|i| i.level == Level::Ok) {
        // the first row carries "host name X · " in front
        let d = it.detail.split(" · ").filter(|p| !p.starts_with("host name")).collect::<Vec<_>>();
        match it.title.as_str() {
            "Ethernet" => parts.push("Ethernet connected".to_string()),
            "Wi-Fi" => parts.push(format!("Wi-Fi: {}", d.get(1).copied().unwrap_or("connected"))),
            _ => {}
        }
    }
    if !parts.is_empty() {
        return (parts.join(" · "), true);
    }
    if snap.items.iter().any(|i| i.title == "Network") {
        return ("network not shown here".into(), false);
    }
    ("no network".into(), false)
}

/// "Battery 85%" (with "charging"), and whether it is fine (not low); `None` without a battery.
pub fn battery_chip(snap: Option<&Snapshot>) -> Option<(String, bool)> {
    let it = snap?.items.iter().find(|i| i.title == "Battery")?;
    let mut p = it.detail.split(" · ");
    let pct = p.next().unwrap_or("");
    let charging = p.next().is_some_and(|s| s == "charging");
    Some((format!("Battery {pct}{}", if charging { " charging" } else { "" }), it.level == Level::Ok))
}

/// Which pieces fit in `avail` and where. Priority when room runs out: Power, time, host name, network, battery, date. Left
/// group (host, network, battery) from the left edge, right group (date, time, Power) against the right edge; because the
/// chosen widths sum to at most `avail`, the two groups never meet.
pub fn plan(avail: f32, s: f32, widths: &[(Chip, f32)]) -> Vec<(Chip, R)> {
    let s = bs(s);
    let (pad, gap) = (10.0 * s, 8.0 * s);
    let order = [Chip::Power, Chip::Time, Chip::Host, Chip::Network, Chip::Battery, Chip::Date];
    let width_of = |c: Chip| widths.iter().find(|(k, _)| *k == c).map(|(_, w)| *w);
    let mut used = 2.0 * pad;
    let mut chosen = Vec::new();
    for c in order {
        if let Some(w) = width_of(c) {
            let need = w + if chosen.is_empty() { 0.0 } else { gap };
            if used + need <= avail {
                used += need;
                chosen.push(c);
            }
        }
    }
    let (y, h) = (4.0 * s, bar_h(s) - 8.0 * s);
    let mut out = Vec::new();
    let mut x = pad;
    for c in [Chip::Host, Chip::Network, Chip::Battery] {
        if chosen.contains(&c) {
            let w = width_of(c).unwrap_or(0.0);
            out.push((c, R::new(x, y, w, h)));
            x += w + gap;
        }
    }
    let mut right = avail - pad;
    for c in [Chip::Power, Chip::Time, Chip::Date] {
        if chosen.contains(&c) {
            let w = width_of(c).unwrap_or(0.0);
            right -= w;
            out.push((c, R::new(right, y, w, h)));
            right -= gap;
        }
    }
    out
}

/// Draw the bar. `power` shows the Power button (never on a remote session). Returns the pieces for tests.
pub fn draw(g: &mut Gfx, w: u32, s: f32, bar: &Bar, power: bool) -> Vec<(Chip, R)> {
    let s = bs(s);
    let bh = bar_h(s);
    g.rect(0.0, 0.0, w as f32, bh, [10, 14, 28, 235]);
    g.rect(0.0, bh - 1.0, w as f32, 1.0, [70, 225, 255, 40]);
    let size = 13.0 * s;
    let dot = 7.0 * s;
    let cap = (w as f32 * 0.4).max(120.0 * s);
    let net = g.fit(size, &bar.net.0, Face::Sans, cap - 2.0 * dot - 16.0 * s);
    let bat = bar.battery.as_ref().map(|b| g.fit(size, &b.0, Face::Sans, cap - 2.0 * dot - 16.0 * s));
    let host = g.fit(size, &bar.host, Face::Bold, cap);
    let tw = |g: &mut Gfx, t: &str, f: Face| g.text_width(size, t, f);
    let mut widths = vec![
        (Chip::Host, tw(g, &host, Face::Bold) + 16.0 * s),
        (Chip::Network, tw(g, &net, Face::Sans) + 2.0 * dot + 16.0 * s),
        (Chip::Time, tw(g, &bar.time, Face::Bold) + 16.0 * s),
        (Chip::Date, tw(g, &bar.date, Face::Sans) + 16.0 * s),
    ];
    if let Some(b) = &bat {
        widths.push((Chip::Battery, tw(g, b, Face::Sans) + 2.0 * dot + 16.0 * s));
    }
    if power {
        widths.push((Chip::Power, tw(g, "Power", Face::Bold) + 16.0 * s + 22.0 * s));
    }
    let placed = plan(w as f32, s, &widths);
    for (c, r) in &placed {
        let ty = r.y + (r.h - g.line_height(size, Face::Sans)) / 2.0;
        match c {
            Chip::Host => {
                g.text(r.x + 8.0 * s, ty, size, MUTED, &host, Face::Bold);
            }
            Chip::Network | Chip::Battery => {
                let (label, ok) = if *c == Chip::Network { (&net, bar.net.1) } else { (bat.as_ref().unwrap(), bar.battery.as_ref().is_some_and(|b| b.1)) };
                g.rrect(r.x + 8.0 * s, r.y + (r.h - dot) / 2.0, dot, dot, dot / 2.0, if ok { OK } else { rgb(255, 196, 94) });
                g.text(r.x + 8.0 * s + dot + 6.0 * s, ty, size, if ok { TEXT } else { MUTED }, label, Face::Sans);
            }
            Chip::Time => {
                g.text(r.x + 8.0 * s, ty, size, TEXT, &bar.time, Face::Bold);
            }
            Chip::Date => {
                g.text(r.x + 8.0 * s, ty, size, MUTED, &bar.date, Face::Sans);
            }
            Chip::Power => {
                let hot = g.hovered(*r);
                g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, if hot { rgb(90, 30, 44) } else { rgb(52, 22, 34) });
                g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.2, ERR);
                g.icon(Icon::Power, r.x + 14.0 * s, r.y + r.h / 2.0, 16.0 * s, ERR);
                g.text(r.x + 26.0 * s, ty, size, TEXT, "Power", Face::Bold);
                g.hit(*r, Event::Key(Key::Ctrl(POWER_KEY)), "power");
            }
        }
    }
    placed
}

/// The card's panel and its buttons (Power off, Restart, [Sleep], Cancel; three or four). Buttons wrap onto further rows when
/// the panel is too narrow, and everything stays inside the screen.
pub fn card_rects(w: f32, h: f32, s: f32, widths: &[f32]) -> (R, Vec<R>) {
    let bw = (500.0 * s).min(w - 16.0);
    let size = 15.0 * s;
    let bh_btn = size * 2.1;
    let inner = bw - 56.0 * s;
    let gap = 10.0 * s;
    let mut rows = 1usize;
    let mut used = 0.0f32;
    let mut place: Vec<(usize, f32)> = vec![];
    for wd in widths {
        if used > 0.0 && used + gap + wd > inner {
            rows += 1;
            used = 0.0;
        }
        place.push((rows - 1, used));
        used += wd + gap;
    }
    let extra = (rows - 1) as f32 * (bh_btn + gap);
    let bh = (170.0 * s + extra).min(h - 8.0);
    let (x, y) = ((w - bw) / 2.0, (h - bh) / 2.0);
    let by = y + bh - 38.0 * s - bh_btn - extra;
    let bx = x + 28.0 * s;
    let rects = widths.iter().zip(&place).map(|(wd, (row, off))| R::new(bx + off, by + *row as f32 * (bh_btn + gap), *wd, bh_btn)).collect();
    (R::new(x, y, bw, bh), rects)
}

/// The power card over the login screen. `sel` is the highlighted choice; `can` false adds the "cannot power off" line;
/// `sleep` adds the Sleep choice (before Cancel). Returns the panel and the button rectangles.
pub fn draw_card(g: &mut Gfx, w: u32, h: u32, s: f32, sel: usize, can: bool, sleep: bool) -> (R, Vec<R>) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 200]);
    g.hit(R::new(0.0, 0.0, w as f32, h as f32), Event::Key(Key::Esc), "backdrop-cancel");
    let size = 15.0 * s;
    let mut items = vec![("Power off", ERR, Event::Key(Key::Char('o'))), ("Restart", TEXT, Event::Key(Key::Char('r')))];
    if sleep {
        items.push(("Sleep", TEXT, Event::Key(Key::Char('s'))));
    }
    items.push(("Cancel", MUTED, Event::Key(Key::Esc)));
    let widths: Vec<f32> = items.iter().map(|i| g.text_width(size, i.0, Face::Bold) + size * 1.7).collect();
    let (panel, rects) = card_rects(w as f32, h as f32, s, &widths);
    g.hit(panel, Event::Tick, "backdrop");
    crate::fx::modal_panel(g, panel, s, ERR, true);
    g.icon(Icon::Power, panel.x + 56.0 * s, panel.y + 52.0 * s, 44.0 * s, ERR);
    g.text(panel.x + 100.0 * s, panel.y + 26.0 * s, 24.0 * s, TEXT, "Power", Face::Bold);
    let sub = if !can { CANNOT } else if sleep { "Shut down, restart or sleep this machine." } else { "Shut down or restart this machine." };
    let sub = g.fit(14.0 * s, sub, Face::Sans, panel.w - 112.0 * s);
    g.text(panel.x + 100.0 * s, panel.y + 62.0 * s, 14.0 * s, if can { MUTED } else { rgb(255, 196, 94) }, &sub, Face::Sans);
    for (i, (label, accent, ev)) in items.iter().enumerate() {
        let r = rects[i];
        if i == sel {
            g.rrect_outline(r.x - 3.0 * s, r.y - 3.0 * s, r.w + 6.0 * s, r.h + 6.0 * s, r.h / 2.0 + 3.0 * s, 2.0, rgb(120, 170, 255));
        }
        g.button(r.x, r.y, size, label, *accent, ev.clone());
    }
    let hint = "Enter or y: the highlighted choice  ·  Esc: cancel";
    let hint = g.fit(12.0 * s, hint, Face::Sans, panel.w - 40.0 * s);
    g.text(panel.x + 20.0 * s, panel.y + panel.h - 26.0 * s, 12.0 * s, DIM, &hint, Face::Sans);
    (panel, rects)
}
