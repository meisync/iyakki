//! The "IYAKKI drive" banner: a persistent card on every screen while the drive the data lives on is lost, failing or read-only
//! (`iyakki_storage::loss`, docs/STORAGE-LOSS.md), and the short "reconnected" card afterwards.
//!
//! While the drive is not healthy the card cannot be dismissed (there is no close button and Esc does not close it); it can be
//! minimised to a slim strip that opens it again. It sits under the top bar, centred, and is drawn above every app and pane. Its
//! buttons: Reconnect now, Restart, Check the drive, Minimise. The geometry is computed first ([`layout`], pure) so it can be tested
//! at every size: the card stays inside the screen and no two buttons overlap.

use crate::fx;
use crate::gfx::{Face, Gfx};
use crate::safeclamp::SafeClamp;
use crate::ui::{Event, ERR, MUTED, TEXT, R};
use iyakki_storage::loss::{self, Banner, Button, Snapshot, State};

/// Click regions.
pub const RECONNECT: usize = 1_350_001;
pub const RESTART: usize = 1_350_002;
pub const CHECK: usize = 1_350_003;
pub const MINIMISE: usize = 1_350_004;
pub const EXPAND: usize = 1_350_005;
pub const CARD_OK: usize = 1_350_006;

/// How long the "reconnected" card stays (heartbeats, about 30 a second).
pub const CARD_TICKS: u64 = 300;

#[derive(Default)]
pub struct LossBar {
    /// Shrunk to a strip.
    pub minimised: bool,
    /// The count of reattaches the shell has already told about.
    pub seen_reattached: u32,
    /// The last state the shell saw (to notice the way back to healthy).
    pub last: Option<State>,
    /// The "reconnected" card shows until this heartbeat.
    pub card_until: Option<u64>,
    pub card_text: String,
    /// Where the banner or strip was drawn last.
    pub rect: Option<R>,
}

impl LossBar {
    /// True while the banner (or its strip) is on screen.
    pub fn showing(&self, s: &Snapshot) -> bool {
        !s.state.healthy()
    }

    /// Feed the current state at heartbeat `tick`. Returns true when the picture changed.
    pub fn update(&mut self, s: &Snapshot, tick: u64) -> bool {
        // the first look at a healthy drive changes nothing on screen (an idle heartbeat must draw nothing)
        let mut changed = match &self.last {
            None => !s.state.healthy(),
            Some(l) => l != &s.state,
        };
        let was_bad = self.last.as_ref().is_some_and(|l| !l.healthy());
        if s.state.healthy() {
            if was_bad {
                self.card_text = if s.reattached > self.seen_reattached { "Drive reconnected, everything is saved again.".into() } else { "The IYAKKI drive works again.".into() };
                self.card_until = Some(tick + CARD_TICKS);
            }
            self.minimised = false;
        } else if self.last.as_ref().is_none_or(|l| l.healthy()) {
            // a new loss: the banner opens in full
            self.minimised = false;
            self.card_until = None;
        }
        self.seen_reattached = s.reattached;
        if self.card_until.is_some_and(|t| tick >= t) {
            self.card_until = None;
            changed = true;
        }
        self.last = Some(s.state.clone());
        changed
    }

    pub fn card_visible(&self) -> bool {
        self.card_until.is_some()
    }

    pub fn dismiss_card(&mut self) {
        self.card_until = None;
    }
}

/// What the layout of the card is.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub card: R,
    pub title: String,
    pub title_at: (f32, f32),
    pub lines: Vec<(f32, f32, String)>,
    /// (click id, label, rect)
    pub buttons: Vec<(usize, String, R)>,
}

fn button_id(b: Button) -> usize {
    match b {
        Button::Reconnect => RECONNECT,
        Button::Restart => RESTART,
        Button::Check => CHECK,
    }
}

fn button_label(b: Button) -> &'static str {
    match b {
        Button::Reconnect => "Reconnect now",
        Button::Restart => "Restart",
        Button::Check => "Check the drive",
    }
}

const TOP: f32 = 64.0;

/// The full card for `b` in a `w` x `h` screen at scale `s`.
pub fn layout(g: &mut Gfx, w: f32, h: f32, s: f32, b: &Banner) -> Layout {
    let margin = 8.0 * s;
    let cw = (760.0 * s).min(w - 2.0 * margin).max(80.0);
    let pad = 16.0 * s;
    let inner = (cw - 2.0 * pad).max(40.0);
    let (tsize, bsize, btn) = (17.0 * s, 13.5 * s, 13.0 * s);
    let (th, bh) = (g.line_height(tsize, Face::Bold), g.line_height(bsize, Face::Sans));
    let title = g.fit(tsize, &b.title, Face::Bold, inner);
    let wrapped = g.wrap(bsize, &b.body, Face::Sans, inner);
    // the buttons flow into rows
    let labels: Vec<(usize, String)> = b.buttons.iter().map(|x| (button_id(*x), button_label(*x).to_string())).chain(std::iter::once((MINIMISE, "Minimise".to_string()))).collect();
    let (btn_h, gap) = (btn * 2.1, 8.0 * s);
    let widths: Vec<f32> = labels.iter().map(|(_, l)| g.text_width(btn, l, Face::Bold) + btn * 1.7).collect();
    let mut rows = 1usize;
    let mut x = 0.0f32;
    let mut placed: Vec<(usize, usize, f32)> = Vec::new();
    for (i, bw) in widths.iter().enumerate() {
        if x > 0.0 && x + bw > inner {
            rows += 1;
            x = 0.0;
        }
        placed.push((i, rows - 1, x));
        x += bw + gap;
    }
    let buttons_h = rows as f32 * (btn_h + gap);
    // as many lines as fit under the top bar
    let fixed = pad + th + 6.0 * s + 8.0 * s + buttons_h + pad;
    let room = (h - TOP * s - margin - fixed).max(0.0);
    let fit_lines = ((room / bh.max(1.0)).floor() as usize).max(1);
    let take = wrapped.len().min(fit_lines).min(9);
    let ch = fixed + take as f32 * bh;
    let x0 = ((w - cw) / 2.0).clamp_safe(0.0, (w - cw).max(0.0));
    let y0 = (TOP * s).clamp_safe(0.0, (h - ch).max(0.0));
    let card = R::new(x0, y0, cw, ch);
    let mut ly = y0 + pad;
    let title_at = (x0 + pad, ly);
    ly += th + 6.0 * s;
    let mut lines = Vec::new();
    for l in wrapped.iter().take(take) {
        lines.push((x0 + pad, ly, l.clone()));
        ly += bh;
    }
    ly += 8.0 * s;
    let buttons = placed.into_iter().map(|(i, row, bx)| (labels[i].0, labels[i].1.clone(), R::new(x0 + pad + bx, ly + row as f32 * (btn_h + gap), widths[i], btn_h))).collect();
    Layout { card, title, title_at, lines, buttons }
}

/// The minimised strip.
pub fn strip(g: &mut Gfx, w: f32, h: f32, s: f32, b: &Banner) -> (R, String) {
    let size = 13.0 * s;
    let max_w = (w - 16.0 * s).max(40.0);
    let text = g.fit(size, &format!("{}  -  show", b.title), Face::Bold, (max_w - 36.0 * s).max(10.0));
    let sw = (g.text_width(size, &text, Face::Bold) + 36.0 * s).min(max_w);
    let sh = size * 2.2;
    let x = ((w - sw) / 2.0).clamp_safe(0.0, (w - sw).max(0.0));
    let y = (TOP * s).clamp_safe(0.0, (h - sh).max(0.0));
    (R::new(x, y, sw, sh), text)
}

/// Draw the banner (or strip, or the reconnected card) over everything. `snap` is the state to show.
pub fn render(bar: &mut LossBar, g: &mut Gfx, w: u32, h: u32, s: f32, snap: &Snapshot) {
    let (wf, hf) = (w as f32, h as f32);
    bar.rect = None;
    let Some(b) = loss::banner(snap) else {
        if bar.card_visible() {
            render_card(bar, g, wf, hf, s);
        }
        return;
    };
    let danger = !matches!(snap.state, State::Reattachable | State::Reconnecting);
    let accent = if danger { ERR } else { fx::AMBER };
    if bar.minimised {
        let (r, text) = strip(g, wf, hf, s, &b);
        fx::modal_panel(g, r, s * 0.5, accent, danger);
        g.text(r.x + 18.0 * s, r.y + (r.h - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, TEXT, &text, Face::Bold);
        g.hit(r, Event::Tab(EXPAND), "IYAKKI drive problem: show");
        bar.rect = Some(r);
        return;
    }
    let l = layout(g, wf, hf, s, &b);
    // the card takes the clicks under it
    g.hit(l.card, Event::Tick, "drive banner");
    fx::modal_panel(g, l.card, s, accent, danger);
    g.text(l.title_at.0, l.title_at.1, 17.0 * s, TEXT, &l.title, Face::Bold);
    for (x, y, t) in &l.lines {
        g.text(*x, *y, 13.5 * s, MUTED, t, Face::Sans);
    }
    for (id, label, r) in &l.buttons {
        let c = if *id == MINIMISE { MUTED } else { accent };
        g.button(r.x, r.y, 13.0 * s, label, c, Event::Tab(*id));
    }
    bar.rect = Some(l.card);
}

fn render_card(bar: &mut LossBar, g: &mut Gfx, w: f32, h: f32, s: f32) {
    let margin = 8.0 * s;
    let cw = (420.0 * s).min(w - 2.0 * margin).max(80.0);
    let pad = 16.0 * s;
    let lines = g.wrap(13.5 * s, &bar.card_text, Face::Sans, (cw - 2.0 * pad).max(40.0));
    let lh = g.line_height(13.5 * s, Face::Sans);
    let btn = 13.0 * s;
    let ch = pad * 2.0 + 19.0 * s + 6.0 * s + lines.len().min(4) as f32 * lh + 8.0 * s + btn * 2.1;
    let x = ((w - cw) / 2.0).clamp_safe(0.0, (w - cw).max(0.0));
    let y = (TOP * s).clamp_safe(0.0, (h - ch).max(0.0));
    let card = R::new(x, y, cw, ch);
    g.hit(card, Event::Tick, "drive card");
    fx::modal_panel(g, card, s, fx::AMBER, false);
    g.text(x + pad, y + pad, 17.0 * s, TEXT, "Drive reconnected", Face::Bold);
    for (i, l) in lines.iter().take(4).enumerate() {
        g.text(x + pad, y + pad + 25.0 * s + i as f32 * lh, 13.5 * s, MUTED, l, Face::Sans);
    }
    g.button(x + pad, y + ch - pad - btn * 2.1, btn, "OK", fx::AMBER, Event::Tab(CARD_OK));
    bar.rect = Some(card);
}

/// Is `n` one of this banner's click regions?
pub fn is_click(n: usize) -> bool {
    (RECONNECT..=CARD_OK).contains(&n)
}
