//! Settings -> Screen saver and screen off (under Display & sound): what the screen does when nobody has touched the machine for a while.
//!
//! One card per choice, with the choices as buttons you click (or Up/Down to pick a card, Enter to take the next choice, `p` to
//! preview). Everyone changes their own choices (saved in their own file, `iyakki_agent::screensaver`); a guest uses the standard
//! ones and can only preview; an administrator also sets what the sign-in screen does. The shell reads the choices again every
//! second, so a change acts at once. The idle clock, the logo and the panel are `saver.rs`, `shell_saver.rs` and `panel.rs`.

use super::*;
use iyakki_agent::screensaver::{self as ss, First, SaverConfig, Style};
use iyakki_sec::policy::Role;

/// The cards, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SsRow {
    First,
    After,
    Then,
    Style,
    Dim,
    Battery,
    Preview,
    SiFirst,
    SiAfter,
    SiThen,
}

const PERSON_ROWS: [SsRow; 7] = [SsRow::First, SsRow::After, SsRow::Then, SsRow::Style, SsRow::Dim, SsRow::Battery, SsRow::Preview];
const ADMIN_ROWS: [SsRow; 10] = [SsRow::First, SsRow::After, SsRow::Then, SsRow::Style, SsRow::Dim, SsRow::Battery, SsRow::Preview, SsRow::SiFirst, SsRow::SiAfter, SsRow::SiThen];

/// One placed card: where it is and where its buttons are.
pub(super) struct SsBox {
    pub row: usize,
    pub card: R,
    /// Each button with the index of its choice (a button that does not fit is left out, so the position is not the index).
    pub chips: Vec<(usize, R)>,
}

fn minutes_label(m: u32, zero: &str) -> String {
    if m == 0 { zero.to_string() } else { format!("{m} min") }
}

impl SettingsApp {
    pub(super) fn ss_rows(admin: bool) -> &'static [SsRow] {
        if admin { &ADMIN_ROWS } else { &PERSON_ROWS }
    }

    pub(super) fn ss_len(cx: &AppCx) -> usize {
        Self::ss_rows(cx.session.policy.manage_settings).len()
    }

    fn ss_guest(cx: &AppCx) -> bool {
        cx.session.role == Role::Guest
    }

    /// This person's choices (the standard ones for a guest).
    pub(super) fn ss_person(cx: &AppCx) -> SaverConfig {
        if Self::ss_guest(cx) { SaverConfig::default() } else { SaverConfig::load(Self::svc(cx).dir(), &cx.session.user) }
    }

    pub(super) fn ss_signin(cx: &AppCx) -> SaverConfig {
        Self::svc(cx).settings.lock().map(|s| s.get().screensaver_signin.0.clone().normalized()).unwrap_or_else(|_| SaverConfig::signin_default())
    }

    fn ss_cfg_for(row: SsRow, cx: &AppCx) -> SaverConfig {
        if matches!(row, SsRow::SiFirst | SsRow::SiAfter | SsRow::SiThen) { Self::ss_signin(cx) } else { Self::ss_person(cx) }
    }

    /// The buttons of a card: their labels and which one is the current choice.
    pub(super) fn ss_chips(row: SsRow, c: &SaverConfig) -> (Vec<String>, Option<usize>) {
        let pos = |v: &[u32], x: u32| v.iter().position(|y| *y == x);
        match row {
            SsRow::First | SsRow::SiFirst => (First::ALL.iter().map(|f| f.label().to_string()).collect(), First::ALL.iter().position(|f| *f == c.first)),
            SsRow::After | SsRow::SiAfter => (ss::AFTER_CHOICES.iter().map(|m| minutes_label(*m, "Never")).collect(), pos(&ss::AFTER_CHOICES, c.after_min)),
            SsRow::Then | SsRow::SiThen => (ss::THEN_CHOICES.iter().map(|m| minutes_label(*m, "Not at all")).collect(), pos(&ss::THEN_CHOICES, c.then_off_min)),
            SsRow::Style => (Style::ALL.iter().map(|s| s.short().to_string()).collect(), Style::ALL.iter().position(|s| *s == c.style)),
            SsRow::Dim => (ss::DIM_CHOICES.iter().map(|p| if *p == 0 { "Do not dim".to_string() } else { format!("{p}%") }).collect(), pos(&ss::DIM_CHOICES, c.dim_pct)),
            SsRow::Battery => (vec!["On: half the time".to_string(), "Off".to_string()], Some(if c.battery_shorter { 0 } else { 1 })),
            SsRow::Preview => (vec!["Preview now".to_string()], None),
        }
    }

    fn ss_text(row: SsRow) -> (&'static str, &'static str) {
        match row {
            SsRow::First => ("When the screen is idle", "What happens when nobody has touched the keyboard, mouse or screen for a while. Sound and running jobs are not stopped."),
            SsRow::After => ("Idle for", "How long without a key, a click or a touch. Music, video, a waiting approval and a warning on screen keep the screen on."),
            SsRow::Then => ("Then turn the screen off after", "More minutes after the screen saver started. Only used with the screen saver."),
            SsRow::Style => ("Screen saver style", "Always on black, always moving slowly. The small logo is the gentlest on OLED screens."),
            SsRow::Dim => ("Dim the screen first", "Laptops: lower the backlight to this level when the screen saver starts, or just before the screen goes off."),
            SsRow::Battery => ("Shorter on battery", "While running on the battery everything above happens after half the time."),
            SsRow::Preview => ("Try it now", "Shows the screen saver for 5 seconds, or until you press a key or move the mouse."),
            SsRow::SiFirst => ("Sign-in screen: when idle", "Before anyone has signed in (administrators). Starts as a screen saver after 5 minutes, off 10 minutes later."),
            SsRow::SiAfter => ("Sign-in screen: idle for", "How long the sign-in screen waits."),
            SsRow::SiThen => ("Sign-in screen: then turn off after", "More minutes after its screen saver started."),
        }
    }

    /// Whether the buttons of a card work for this person.
    fn ss_enabled(row: SsRow, c: &SaverConfig, cx: &AppCx) -> bool {
        match row {
            SsRow::Preview => true,
            SsRow::SiFirst | SsRow::SiAfter | SsRow::SiThen => cx.session.policy.manage_settings && (row != SsRow::SiThen || c.first == First::Saver),
            SsRow::Then => !Self::ss_guest(cx) && c.first == First::Saver,
            _ => !Self::ss_guest(cx),
        }
    }

    /// Put every card in place for the room `a`: cards that do not fit are left out (the selected one always comes into view),
    /// and buttons that would fall outside `a` are not placed.
    pub(super) fn ss_layout(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) -> Vec<SsBox> {
        let rows = Self::ss_rows(cx.session.policy.manage_settings);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let (pad, chip_h, gap) = (12.0 * s, 30.0 * s, 8.0 * s);
        let inner_w = (a.w - 36.0 * s).max(40.0 * s);
        let size = 13.0 * s;
        // each card's height
        let mut heights = Vec::new();
        let mut chip_w: Vec<Vec<f32>> = Vec::new();
        for &row in rows {
            let c = Self::ss_cfg_for(row, cx);
            let (labels, _) = Self::ss_chips(row, &c);
            let ws: Vec<f32> = labels.iter().map(|l| g.text_width(size, l, Face::Bold) + 22.0 * s).collect();
            let (mut lines, mut x) = (1usize, 0.0f32);
            for w in &ws {
                if x > 0.0 && x + w > inner_w {
                    lines += 1;
                    x = 0.0;
                }
                x += w + gap;
            }
            let help_n = g.wrap(size, Self::ss_text(row).1, Face::Sans, inner_w).len().min(2).max(1);
            heights.push(pad + 22.0 * s + help_n as f32 * 17.0 * s + 6.0 * s + lines as f32 * (chip_h + gap) + pad * 0.5);
            chip_w.push(ws);
        }
        // the first card shown: a click leaves the cards where they are, a key scrolls to the selected one, the wheel scrolls
        let gap_rows = 8.0 * s;
        let steps: Vec<f32> = heights.iter().map(|h| h + gap_rows).collect();
        let first = self.win[self.tab].layout(sel, &steps, a.h + gap_rows);
        let mut out = Vec::new();
        let mut y = a.y;
        for i in first..rows.len() {
            if y + heights[i] > a.y + a.h + 0.5 {
                break;
            }
            let card = R::new(a.x, y, a.w, heights[i]);
            let help_n = g.wrap(size, Self::ss_text(rows[i]).1, Face::Sans, inner_w).len().min(2).max(1);
            let mut cy = y + pad + 22.0 * s + help_n as f32 * 17.0 * s + 6.0 * s;
            let mut cx_ = a.x + 18.0 * s;
            let mut chips = Vec::new();
            for (n, w) in chip_w[i].iter().enumerate() {
                if cx_ > a.x + 18.0 * s && cx_ + w > a.x + 18.0 * s + inner_w {
                    cx_ = a.x + 18.0 * s;
                    cy += chip_h + gap;
                }
                let r = R::new(cx_, cy, *w, chip_h);
                if r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5 {
                    chips.push((n, r));
                }
                cx_ += w + gap;
            }
            out.push(SsBox { row: i, card, chips });
            y += heights[i] + gap_rows;
        }
        out
    }

    pub(super) fn render_screensaver(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let guest = Self::ss_guest(cx);
        let rows = Self::ss_rows(cx.session.policy.manage_settings);
        let mut top = a.y;
        if guest {
            let t = g.fit(13.0 * s, "Guests use the standard choices (screen saver after 10 minutes). You can still preview.", Face::Sans, a.w);
            g.text(a.x, top, 13.0 * s, DIM, &t, Face::Sans);
            top += 22.0 * s;
        }
        let body = R::new(a.x, top, a.w, (a.y + a.h - top).max(0.0));
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let size = 13.0 * s;
        for b in self.ss_layout(g, body, s, cx) {
            let row = rows[b.row];
            let c = Self::ss_cfg_for(row, cx);
            let enabled = Self::ss_enabled(row, &c, cx);
            let hot = g.hovered(b.card);
            crate::fx::row_at(g, b.card.x, b.card.y, b.card.w, b.card.h, 14.0 * s, false, hot, ACCENT);
            g.hit(b.card, Event::Row(b.row), "card");
            let (title, help) = Self::ss_text(row);
            let text_w = (b.card.w - 36.0 * s).max(40.0 * s);
            let title = g.fit(16.0 * s, title, Face::Bold, text_w);
            g.text(b.card.x + 18.0 * s, b.card.y + 12.0 * s, 16.0 * s, if enabled { TEXT } else { MUTED }, &title, Face::Bold);
            for (n, l) in g.wrap(size, help, Face::Sans, text_w).iter().take(2).enumerate() {
                g.text(b.card.x + 18.0 * s, b.card.y + 12.0 * s + 22.0 * s + n as f32 * 17.0 * s, size, DIM, l, Face::Sans);
            }
            let (labels, chosen) = Self::ss_chips(row, &c);
            for (i, r) in b.chips.iter().map(|(i, r)| (*i, r)) {
                let on = chosen == Some(i);
                let over = enabled && g.hovered(*r);
                let fill = if on { ACCENT } else if over { crate::ui::CARD_SEL } else { crate::ui::CARD_HOVER };
                g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, if enabled || on { fill } else { crate::ui::CARD });
                let Some(label) = labels.get(i) else { continue };
                let lw = g.text_width(size, label, Face::Bold);
                let ink = if on { rgb(10, 14, 28) } else if enabled { TEXT } else { MUTED };
                g.text(r.x + (r.w - lw) / 2.0, r.y + (r.h - g.line_height(size, Face::Bold)) / 2.0, size, ink, label, Face::Bold);
                if enabled {
                    g.hit(*r, Event::Sub(b.row.min(255) as u8, Key::Char(char::from(b'a' + i.min(25) as u8))), label);
                }
            }
            if b.row == sel {
                g.rrect_outline(b.card.x, b.card.y, b.card.w, b.card.h, 14.0 * s, 2.0 * s, ACCENT);
            }
        }
    }

    pub(super) fn screensaver_buttons(&self) -> Vec<Btn> {
        vec![("Next choice", ACCENT, 'n'), ("Previous", TEXT, 'N'), ("Preview", ACCENT, 'p')]
    }

    pub(super) fn handle_screensaver(&mut self, k: &Key, cx: &AppCx) {
        let len = Self::ss_len(cx);
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Home => self.sel[self.tab] = 0,
            Key::End => self.sel[self.tab] = len.saturating_sub(1),
            Key::Enter | Key::Char(' ' | 'n' | '+' | '=') => self.ss_step(self.sel(), 1, cx),
            Key::Char('N' | '-') => self.ss_step(self.sel(), -1, cx),
            Key::Char('p') => self.ss_preview(cx),
            _ => {}
        }
    }

    /// A click on a button of a card.
    pub(super) fn ss_sub(&mut self, row: usize, c: char, cx: &AppCx) {
        let rows = Self::ss_rows(cx.session.policy.manage_settings);
        let Some(&r) = rows.get(row) else { return };
        self.sel[self.tab] = row;
        let idx = (c as u32).saturating_sub('a' as u32) as usize;
        self.ss_choose(r, idx, cx);
    }

    /// The next (or previous) choice of the selected card.
    fn ss_step(&mut self, row: usize, delta: i32, cx: &AppCx) {
        let rows = Self::ss_rows(cx.session.policy.manage_settings);
        let Some(&r) = rows.get(row) else { return };
        let c = Self::ss_cfg_for(r, cx);
        let (labels, chosen) = Self::ss_chips(r, &c);
        let n = labels.len().max(1) as i32;
        let next = (chosen.map_or(-1, |i| i as i32) + delta).rem_euclid(n) as usize;
        self.ss_choose(r, next, cx);
    }

    fn ss_choose(&mut self, row: SsRow, idx: usize, cx: &AppCx) {
        if row == SsRow::Preview {
            return self.ss_preview(cx);
        }
        let mut c = Self::ss_cfg_for(row, cx);
        if !Self::ss_enabled(row, &c, cx) {
            if Self::ss_guest(cx) {
                self.ok("Guests use the standard choices.");
            } else if matches!(row, SsRow::Then | SsRow::SiThen) {
                self.ok("This only applies with the screen saver.");
            } else {
                self.ok("Only administrators change the sign-in screen.");
            }
            return;
        }
        let what = match row {
            SsRow::First | SsRow::SiFirst => First::ALL.get(idx).map(|f| {
                c.first = *f;
                format!("when idle: {}", f.label())
            }),
            SsRow::After | SsRow::SiAfter => ss::AFTER_CHOICES.get(idx).map(|m| {
                c.after_min = *m;
                format!("idle for: {}", minutes_label(*m, "never"))
            }),
            SsRow::Then | SsRow::SiThen => ss::THEN_CHOICES.get(idx).map(|m| {
                c.then_off_min = *m;
                format!("then off after: {}", minutes_label(*m, "not at all"))
            }),
            SsRow::Style => Style::ALL.get(idx).map(|st| {
                c.style = *st;
                format!("style: {}", st.label())
            }),
            SsRow::Dim => ss::DIM_CHOICES.get(idx).map(|p| {
                c.dim_pct = *p;
                format!("dim: {p}%")
            }),
            SsRow::Battery => (idx < 2).then(|| {
                c.battery_shorter = idx == 0;
                format!("shorter on battery: {}", if idx == 0 { "on" } else { "off" })
            }),
            SsRow::Preview => None,
        };
        let Some(what) = what else { return };
        let c = c.normalized();
        let signin = matches!(row, SsRow::SiFirst | SsRow::SiAfter | SsRow::SiThen);
        let saved = if signin {
            Self::svc(cx).settings.lock().map_err(|_| "The settings are busy; try again.".to_string()).and_then(|mut st| st.update(|s| s.screensaver_signin = iyakki_agent::settings::SigninSaver(c.clone())).map(|_| ()).map_err(|e| e.to_string()))
        } else {
            c.save(Self::svc(cx).dir(), &cx.session.user)
        };
        match saved {
            Ok(()) => {
                cx.audit.record(&cx.session.user, if signin { "settings.screensaver.signin" } else { "settings.screensaver" }, &what);
                self.ok("Saved.");
            }
            Err(e) => self.fail(e),
        }
    }

    fn ss_preview(&mut self, cx: &AppCx) {
        iyakki_agent::screensaver::request(Self::svc(cx).dir(), &cx.session.user, iyakki_agent::screensaver::Request::Preview);
        self.ok("Showing the screen saver for 5 seconds. Press a key or move the mouse to stop it.");
    }
}
