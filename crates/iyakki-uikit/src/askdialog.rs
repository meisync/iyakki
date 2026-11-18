//! The speaker question of a player that wants the sound card while another holds it: the three answers (keys and clicks) and the dialog that draws the
//! question with Stop it / Keep it / Cancel. Pure toolkit code (no player, no registry): the shell's `apps/soundask.rs` keeps who holds the card and how
//! to stop it, and the radio tile crate draws the same dialog from here (docs/development/RADIO-EXTRACTION-PLAN.md step 3).

use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, MUTED, R, TEXT};

/// Button ids (`Event::Tab`) in the browser; the radio uses keys.
pub const TAB_STOP: usize = 14;
pub const TAB_KEEP: usize = 15;
pub const TAB_CANCEL: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stop,
    Keep,
    Cancel,
}

/// y or Enter stops it, n or k keeps it, Esc cancels.
pub fn answer_for_key(k: &Key) -> Option<Answer> {
    match k {
        Key::Char('y') | Key::Enter => Some(Answer::Stop),
        Key::Char('n') | Key::Char('k') => Some(Answer::Keep),
        Key::Esc => Some(Answer::Cancel),
        _ => None,
    }
}

pub fn answer_for_tab(n: usize) -> Option<Answer> {
    match n {
        TAB_STOP => Some(Answer::Stop),
        TAB_KEEP => Some(Answer::Keep),
        TAB_CANCEL => Some(Answer::Cancel),
        _ => None,
    }
}

/// The same dialog from a question and a flag (the radio keeps its own state and only draws).
#[allow(clippy::too_many_arguments)]
pub fn render_question(g: &mut Gfx, area: R, s: f32, question: &str, stopping: bool, stop: Event, keep: Event, cancel: Event) {
    let accent = rgb(110, 190, 255);
    let p = crate::fx::modal(g, area, s, 560.0 * s, 210.0 * s, accent, false);
    for (i, l) in g.wrap(18.0 * s, question, Face::Bold, p.w - 48.0 * s).into_iter().take(3).enumerate() {
        g.text(p.x + 24.0 * s, p.y + 24.0 * s + i as f32 * 26.0 * s, 18.0 * s, TEXT, &l, Face::Bold);
    }
    let hint = if stopping { "Stopping..." } else { "y or Enter stops it, n keeps it, Esc cancels." };
    g.text(p.x + 24.0 * s, p.y + 104.0 * s, 14.0 * s, MUTED, hint, Face::Sans);
    let y = p.y + p.h - 58.0 * s;
    if stopping {
        return;
    }
    let mut x = p.x + 24.0 * s;
    x += g.button(x, y, 14.0 * s, "Stop it", accent, stop) + 10.0 * s;
    x += g.button(x, y, 14.0 * s, "Keep it", rgb(150, 160, 180), keep) + 10.0 * s;
    g.button(x, y, 14.0 * s, "Cancel", rgb(150, 160, 180), cancel);
}

