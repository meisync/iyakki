//! The developer-mode approval, on every screen: while a paired computer waits for the administrator to allow it to run commands
//! (restart, patch) the DEV sign in the top bar pulses and a slim banner under the bar pulses with it. A click on the sign or the
//! banner opens a card right there with the choices (Approve for one session, Approve 10 min + auto 4 h, Deny), so nobody has to
//! go to Settings -> Developer mode in the two minutes a request waits. A request to run as root, to power or to patch is never
//! approved automatically: its card has no auto button. The geometry is computed first ([`layout`], pure) so it can be tested at
//! every size.

use crate::fx;
use crate::gfx::{Face, Gfx};
use crate::safeclamp::SafeClamp;
use crate::ui::{Event, ERR, MUTED, TEXT, R};
use iyakki_agent::devmode::grants::ShellRequest;

/// Click regions.
pub const OPEN: usize = 1_360_001;
pub const ONCE: usize = 1_360_002;
pub const AUTO: usize = 1_360_003;
pub const DENY: usize = 1_360_004;
pub const CLOSE: usize = 1_360_005;

pub fn is_click(n: usize) -> bool {
    (OPEN..=CLOSE).contains(&n)
}

/// How often the request list is read (heartbeats, about 30 a second) and how often a pending request redraws the screen.
pub const POLL_EVERY: u64 = 10;
pub const FRAME_EVERY: u64 = 3;
/// The length of one pulse (heartbeats).
const PERIOD: u64 = 24;

const TOP: f32 = 64.0;

#[derive(Default)]
pub struct DevAsk {
    /// The oldest request waiting for the administrator (None: nothing waits, nothing animates).
    pub pending: Option<ShellRequest>,
    /// The card is open.
    pub open: bool,
    /// Where the card or banner was drawn last (it takes the clicks under it).
    pub rect: Option<R>,
}

impl DevAsk {
    /// 0.0 while nothing waits, else a value in 0.25..=1.0 that swells and fades.
    pub fn pulse(&self, tick: u64) -> f32 {
        if self.pending.is_none() {
            return 0.0;
        }
        let t = (tick % PERIOD) as f32 / PERIOD as f32;
        0.25 + 0.75 * (0.5 - 0.5 * (t * std::f32::consts::TAU).cos())
    }

    /// Takes the list read from the device; true when the picture changes. The card closes when its request is gone.
    pub fn update(&mut self, list: Vec<ShellRequest>) -> bool {
        let next = list.into_iter().min_by_key(|r| r.at);
        if next.is_none() {
            self.open = false;
            self.rect = None;
        }
        let changed = next != self.pending;
        self.pending = next;
        changed
    }
}

/// What the person is told the computer wants.
pub fn what(r: &ShellRequest) -> String {
    let doing = match r.kind.as_str() {
        "power" => "restart or switch off this device",
        "patch" => "patch the running system",
        _ => "run commands on this device",
    };
    format!("{} asks to {} as {}{}", r.computer, doing, r.account, if r.as_root { " (as root)" } else { "" })
}

/// The words for what a grant of `kind` allows.
pub fn doing(kind: &str) -> &'static str {
    match kind {
        "power" => "restart or switch off the device",
        "patch" => "patch the running system",
        _ => "run commands",
    }
}

/// Can this request be approved "and stop asking for 4 hours"? Never as root, and only a plain shell.
pub fn auto_allowed(r: &ShellRequest) -> bool {
    r.kind == "shell" && !r.as_root
}

pub struct Layout {
    pub card: R,
    pub title_at: (f32, f32),
    pub lines: Vec<(f32, f32, String)>,
    /// (click id, label, rectangle, enabled)
    pub buttons: Vec<(usize, String, R, bool)>,
    pub close: R,
}

pub fn layout(g: &mut Gfx, w: f32, h: f32, s: f32, r: &ShellRequest) -> Layout {
    let margin = 8.0 * s;
    let cw = (460.0 * s).min(w - 2.0 * margin).max(80.0);
    let pad = 16.0 * s;
    let inner = (cw - 2.0 * pad).max(40.0);
    let (tsize, bsize, btn) = (16.0 * s, 13.5 * s, 13.0 * s);
    let th = g.line_height(tsize, Face::Bold);
    let bh = g.line_height(bsize, Face::Sans);
    let mut body = String::new();
    if !r.why.is_empty() {
        body.push_str(&r.why);
        body.push_str("  ");
    }
    body.push_str(&format!("Asks for {} min.", r.minutes));
    if !auto_allowed(r) {
        body.push_str(if r.as_root { " A request to run as root is always asked each time." } else { " This kind of request is always asked each time." });
    }
    let wrapped = g.wrap(bsize, &body, Face::Sans, inner);
    let title = g.wrap(tsize, &what(r), Face::Bold, inner - 28.0 * s);
    let labels: Vec<(usize, &str, bool)> = vec![(ONCE, "Approve", true), (AUTO, "Approve 10 min + auto 4 h", auto_allowed(r)), (DENY, "Deny", true)];
    let (btn_h, gap) = (btn * 2.1, 8.0 * s);
    let widths: Vec<f32> = labels.iter().map(|(_, l, _)| g.text_width(btn, l, Face::Bold) + btn * 1.7).collect();
    let (mut rows, mut x) = (1usize, 0.0f32);
    let mut placed = Vec::new();
    for (i, bw) in widths.iter().enumerate() {
        if x > 0.0 && x + bw > inner {
            rows += 1;
            x = 0.0;
        }
        placed.push((i, rows - 1, x));
        x += bw + gap;
    }
    let buttons_h = rows as f32 * (btn_h + gap);
    let fixed = pad + 6.0 * s + 8.0 * s + buttons_h + pad;
    let room = (h - TOP * s - margin - fixed - th).max(0.0);
    let lines_fit = (room / bh.max(1.0)).floor() as usize;
    let tl = title.len().clamp(1, 3);
    let take = wrapped.len().min(lines_fit.saturating_sub(tl.saturating_sub(1))).min(5);
    let ch = fixed + tl as f32 * th + take as f32 * bh;
    let x0 = ((w - cw) / 2.0).clamp_safe(0.0, (w - cw).max(0.0));
    let y0 = (TOP * s).clamp_safe(0.0, (h - ch).max(0.0));
    let card = R::new(x0, y0, cw, ch);
    let mut ly = y0 + pad;
    let title_at = (x0 + pad, ly);
    ly += tl as f32 * th + 6.0 * s;
    let mut lines = Vec::new();
    for l in wrapped.iter().take(take) {
        lines.push((x0 + pad, ly, l.clone()));
        ly += bh;
    }
    ly += 8.0 * s;
    let buttons = placed.into_iter().map(|(i, row, bx)| (labels[i].0, labels[i].1.to_string(), R::new(x0 + pad + bx, ly + row as f32 * (btn_h + gap), widths[i], btn_h), labels[i].2)).collect();
    let cs = 22.0 * s;
    let close = R::new(x0 + cw - cs - 8.0 * s, y0 + 8.0 * s, cs, cs);
    Layout { card, title_at, lines, buttons, close }
}

/// The slim banner that pulses while the card is closed.
pub fn strip(g: &mut Gfx, w: f32, h: f32, s: f32, r: &ShellRequest) -> (R, String) {
    let size = 13.0 * s;
    let max_w = (w - 16.0 * s).max(40.0);
    let text = g.fit(size, &format!("{}  -  click to answer", what(r)), Face::Bold, (max_w - 36.0 * s).max(10.0));
    let sw = (g.text_width(size, &text, Face::Bold) + 36.0 * s).min(max_w);
    let sh = size * 2.2;
    let x = ((w - sw) / 2.0).clamp_safe(0.0, (w - sw).max(0.0));
    let y = (TOP * s).clamp_safe(0.0, (h - sh).max(0.0));
    (R::new(x, y, sw, sh), text)
}

pub fn render(ask: &mut DevAsk, g: &mut Gfx, w: u32, h: u32, s: f32, tick: u64) {
    ask.rect = None;
    let Some(r) = ask.pending.clone() else { return };
    let (wf, hf) = (w as f32, h as f32);
    let pulse = ask.pulse(tick);
    let accent = if r.as_root { ERR } else { fx::AMBER };
    if !ask.open {
        let (b, text) = strip(g, wf, hf, s, &r);
        g.rrect_glow(b.x, b.y, b.w, b.h, 12.0 * s, (4.0 + 10.0 * pulse) * s, fx::with_alpha(accent, (40.0 + 150.0 * pulse) as u8));
        g.rrect(b.x, b.y, b.w, b.h, 12.0 * s, rgb_mix(pulse));
        g.rrect_outline(b.x, b.y, b.w, b.h, 12.0 * s, 1.6 * s, fx::with_alpha(accent, (110.0 + 145.0 * pulse) as u8));
        g.text(b.x + 18.0 * s, b.y + (b.h - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, TEXT, &text, Face::Bold);
        g.hit(b, Event::Tab(OPEN), "developer mode approval: answer");
        ask.rect = Some(b);
        return;
    }
    let l = layout(g, wf, hf, s, &r);
    g.hit(l.card, Event::Tick, "developer mode approval");
    fx::modal_panel(g, l.card, s, accent, r.as_root);
    g.rrect_outline(l.card.x, l.card.y, l.card.w, l.card.h, 18.0 * s, 1.6 * s, fx::with_alpha(accent, (120.0 + 135.0 * pulse) as u8));
    let tsize = 16.0 * s;
    let th = g.line_height(tsize, Face::Bold);
    for (i, t) in g.wrap(tsize, &what(&r), Face::Bold, l.card.w - 2.0 * 16.0 * s - 28.0 * s).iter().take(3).enumerate() {
        g.text(l.title_at.0, l.title_at.1 + i as f32 * th, tsize, TEXT, t, Face::Bold);
    }
    for (x, y, t) in &l.lines {
        g.text(*x, *y, 13.5 * s, MUTED, t, Face::Sans);
    }
    for (id, label, rect, enabled) in &l.buttons {
        if *enabled {
            let c = if *id == DENY { ERR } else { accent };
            g.button(rect.x, rect.y, 13.0 * s, label, c, Event::Tab(*id));
        } else {
            g.button_off(rect.x, rect.y, 13.0 * s, label);
        }
    }
    g.text(l.close.x + 5.0 * s, l.close.y, 16.0 * s, MUTED, "x", Face::Bold);
    g.hit(l.close, Event::Tab(CLOSE), "developer mode approval: close");
    ask.rect = Some(l.card);
}

fn rgb_mix(p: f32) -> [u8; 4] {
    [(24.0 + 40.0 * p) as u8, (22.0 + 14.0 * p) as u8, (30.0 - 6.0 * p) as u8, 255]
}
