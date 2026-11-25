//! The dialog that asks the person about the assistant's use of their browser (docs/BROWSER-AGENT.md): may it look at a site, are you willing
//! to let it act for you there (approve all / one by one / deny), may it do this one thing, can you do something only you can (a code, a
//! captcha) and press Continue. The INBOX (Approvals page) and the open chat both show the questions with this dialog; the tile crate owns the
//! drawing and the key map, the shell gives the answer to the waiting tool (`apps/browser_consent.rs`).

use iyakki_tile_api::inbox::{Answer, ConsentKind, ConsentRequest};
use iyakki_uikit::gfx::{rgb, Face, Gfx};
use iyakki_uikit::ui::{Event, Key, R, DIM, ERR, MUTED, TEXT};

const AMBER: [u8; 4] = rgb(255, 170, 70);

#[derive(Clone, Debug)]
pub struct ConsentDialog {
    pub req: ConsentRequest,
}

impl ConsentDialog {
    pub fn new(req: ConsentRequest) -> ConsentDialog {
        ConsentDialog { req }
    }

    /// The answer a key stands for: 1, 2, 3 choose by position; Esc is the "no" (Deny or Cancel) of the question.
    pub fn answer_of(&self, k: &Key) -> Option<Answer> {
        match k {
            Key::Char(c @ '1'..='4') => self.req.choices.get(*c as usize - '1' as usize).map(|c| c.answer),
            Key::Esc if self.req.kind == ConsentKind::Permission => Some(Answer::NotNow),
            Key::Esc => self.req.choices.iter().find(|c| c.answer == Answer::Deny).map(|c| c.answer),
            _ => None,
        }
    }

    pub fn render(&self, g: &mut Gfx, area: R, s: f32) {
        let accent = match self.req.kind {
            ConsentKind::Look => rgb(110, 190, 255),
            ConsentKind::Interact | ConsentKind::Step => AMBER,
            ConsentKind::Handover => rgb(167, 116, 255),
            ConsentKind::Permission => rgb(255, 120, 200),
        };
        if self.req.kind == ConsentKind::Permission {
            return self.render_permission(g, area, s, accent);
        }
        let p = iyakki_uikit::fx::modal(g, area, s, 640.0 * s, 330.0 * s, accent, false);
        let y = iyakki_uikit::fx::modal_title(g, p, s, &self.req.title, accent);
        let mut y = y + 6.0 * s;
        let text = if self.req.kind == ConsentKind::Look { "The assistant wants to read and take pictures of the pages you have open on this site, and to scroll them. It will not click or type." } else { self.req.text.as_str() };
        for line in text.lines() {
            for l in g.wrap(15.0 * s, line, Face::Sans, p.w - 52.0 * s) {
                g.text(p.x + 26.0 * s, y, 15.0 * s, TEXT, &l, Face::Sans);
                y += 21.0 * s;
            }
        }
        if !self.req.detail.is_empty() && self.req.kind != ConsentKind::Step {
            y += 4.0 * s;
            for l in g.wrap(13.5 * s, &self.req.detail, Face::Mono, p.w - 52.0 * s).into_iter().take(3) {
                g.text(p.x + 26.0 * s, y, 13.5 * s, MUTED, &l, Face::Mono);
                y += 19.0 * s;
            }
        }
        // the choices, one button each, numbered
        let mut by = p.y + p.h - 28.0 * s - 40.0 * s * self.req.choices.len() as f32;
        by = by.max(y + 6.0 * s);
        for (i, c) in self.req.choices.iter().enumerate() {
            let deny = c.answer == Answer::Deny;
            let label = format!("{}  {}", i + 1, c.label);
            let w = (g.text_width(14.5 * s, &label, Face::Bold) + 40.0 * s).min(p.w - 52.0 * s);
            g.button(p.x + 26.0 * s, by + i as f32 * 40.0 * s, 14.5 * s, &label, if deny { ERR } else { accent }, Event::Key(Key::Char((b'1' + i as u8) as char)));
            let _ = w;
        }
        g.text(p.x + 26.0 * s, p.y + p.h - 22.0 * s, 12.5 * s, DIM, "Press 1, 2 or 3, or Esc to say no.", Face::Sans);
    }
}

impl ConsentDialog {
    /// A flow's permission request: the exact text of the permission, the flow and step, the model, and four answers.
    fn render_permission(&self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4]) {
        let p = iyakki_uikit::fx::modal(g, area, s, 760.0 * s, 580.0 * s, accent, false);
        let y = iyakki_uikit::fx::modal_title(g, p, s, &self.req.title, accent);
        let mut y = y + 4.0 * s;
        let buttons_h = 2.0 * 42.0 * s + 40.0 * s;
        let limit = p.y + p.h - buttons_h;
        'text: for line in self.req.text.lines() {
            for l in g.wrap(14.0 * s, line, Face::Sans, p.w - 52.0 * s) {
                if y + 19.0 * s > limit {
                    g.text(p.x + 26.0 * s, y, 14.0 * s, MUTED, "…", Face::Sans);
                    break 'text;
                }
                g.text(p.x + 26.0 * s, y, 14.0 * s, TEXT, &l, Face::Sans);
                y += 19.0 * s;
            }
            y += 4.0 * s;
        }
        let colw = (p.w - 52.0 * s - 12.0 * s) / 2.0;
        let by = p.y + p.h - buttons_h + 8.0 * s;
        for (i, c) in self.req.choices.iter().enumerate() {
            let (col, row) = (i % 2, i / 2);
            let block = c.answer == Answer::Block;
            let label = format!("{}  {}", i + 1, g.fit(14.0 * s, &c.label, Face::Bold, colw - 60.0 * s));
            g.button(p.x + 26.0 * s + col as f32 * (colw + 12.0 * s), by + row as f32 * 42.0 * s, 14.0 * s, &label, if block { ERR } else { accent }, Event::Key(Key::Char((b'1' + i as u8) as char)));
        }
        g.text(p.x + 26.0 * s, p.y + p.h - 28.0 * s, 12.0 * s, DIM, "Press 1 to 4. Esc is Not now. Only you can answer this, on this device.", Face::Sans);
    }
}

/// The row text for the Approvals list.
pub fn row_summary(r: &ConsentRequest) -> String {
    match r.kind {
        ConsentKind::Look => format!("The assistant wants to read pages on {} in your browser. It will not click or type.", r.domain),
        ConsentKind::Interact => r.text.clone(),
        ConsentKind::Step => r.text.clone(),
        ConsentKind::Handover => r.text.clone(),
        ConsentKind::Permission => format!("{}. Open it to read what it allows and to answer.", r.title),
    }
}

