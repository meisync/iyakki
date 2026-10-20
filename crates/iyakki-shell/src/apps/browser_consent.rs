//! The dialog that asks the person about the assistant's use of their browser (docs/BROWSER-AGENT.md): may it look at a site, are
//! you willing to let it act for you there (approve all / one by one / deny), may it do this one thing, can you do something
//! only you can (a code, a captcha) and press Continue. The questions are held by `iyakki_agent::browserconsent`; the INBOX
//! (Approvals page) and the open chat both show them with this dialog, so an answer in either place ends the wait.

use iyakki_agent::browserconsent::{Answer, ConsentKind, ConsentRequest};

use super::AppCx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, R, DIM, ERR, MUTED, TEXT};

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
            Key::Char(c @ '1'..='3') => self.req.choices.get(*c as usize - '1' as usize).map(|c| c.answer),
            Key::Esc => self.req.choices.iter().find(|c| c.answer == Answer::Deny).map(|c| c.answer),
            _ => None,
        }
    }

    /// Give the answer to the waiting tool; audited by what it allows, never by the page's content.
    pub fn answer(&self, cx: &AppCx, a: Answer) -> Result<(), String> {
        let r = cx.services.browser.consent.resolve(&cx.session.user, &self.req.id, a);
        if r.is_ok() {
            let what = match (self.req.kind, a) {
                (ConsentKind::Look, Answer::Allow) => "look allowed for 5 minutes",
                (ConsentKind::Look, Answer::AllowSession) => "look allowed for the session",
                (ConsentKind::Look, _) => "look denied",
                (ConsentKind::Interact, Answer::ApproveAll) => "approve all for the session",
                (ConsentKind::Interact, Answer::OneByOne) => "approve one by one",
                (ConsentKind::Interact, _) => "interaction denied",
                (ConsentKind::Step, Answer::Allow | Answer::AllowSession) => "action allowed",
                (ConsentKind::Step, _) => "action denied",
                (ConsentKind::Handover, Answer::Continue) => "handed back after the person's step",
                (ConsentKind::Handover, _) => "hand-over cancelled",
            };
            cx.audit.record(&cx.session.user, "browser.agent.answer", &format!("{} {}", self.req.domain, what));
        }
        r
    }

    pub fn render(&self, g: &mut Gfx, area: R, s: f32) {
        let accent = match self.req.kind {
            ConsentKind::Look => rgb(110, 190, 255),
            ConsentKind::Interact | ConsentKind::Step => AMBER,
            ConsentKind::Handover => rgb(167, 116, 255),
        };
        let p = crate::fx::modal(g, area, s, 640.0 * s, 330.0 * s, accent, false);
        let y = crate::fx::modal_title(g, p, s, &self.req.title, accent);
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

/// The row text for the Approvals list.
pub fn row_summary(r: &ConsentRequest) -> String {
    match r.kind {
        ConsentKind::Look => format!("The assistant wants to read pages on {} in your browser. It will not click or type.", r.domain),
        ConsentKind::Interact => r.text.clone(),
        ConsentKind::Step => r.text.clone(),
        ConsentKind::Handover => r.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_agent::browserconsent::{interact_choices, willingness_text};

    fn req(kind: ConsentKind) -> ConsentRequest {
        let cx = cx_with_admin_store("consentdlg");
        let id = cx.services.browser.consent.create("root", kind, "a.test", &willingness_text("a.test", None), "click button \"Send\"", 1).unwrap();
        cx.services.browser.consent.get("root", &id).unwrap()
    }

    #[test]
    fn keys_choose_by_position_and_esc_is_no() {
        let d = ConsentDialog::new(req(ConsentKind::Interact));
        assert_eq!(d.answer_of(&Key::Char('1')), Some(Answer::ApproveAll));
        assert_eq!(d.answer_of(&Key::Char('2')), Some(Answer::OneByOne));
        assert_eq!(d.answer_of(&Key::Char('3')), Some(Answer::Deny));
        assert_eq!(d.answer_of(&Key::Esc), Some(Answer::Deny));
        assert_eq!(d.answer_of(&Key::Char('4')), None);
        let l = ConsentDialog::new(req(ConsentKind::Look));
        assert_eq!((l.answer_of(&Key::Char('1')), l.answer_of(&Key::Char('2')), l.answer_of(&Key::Char('3'))), (Some(Answer::Allow), Some(Answer::AllowSession), Some(Answer::Deny)));
        assert_eq!(l.answer_of(&Key::Char('4')), None);
        let h = ConsentDialog::new(req(ConsentKind::Handover));
        assert_eq!((h.answer_of(&Key::Char('1')), h.answer_of(&Key::Esc)), (Some(Answer::Continue), Some(Answer::Deny)));
    }

    #[test]
    fn the_dialog_shows_the_willingness_text_and_exactly_the_three_choices() {
        let cx = cx_with_admin_store("consentdlg2");
        let id = cx.services.browser.consent.create("root", ConsentKind::Interact, "gemini.google.com", &willingness_text("gemini.google.com", None), "", 1).unwrap();
        let d = ConsentDialog::new(cx.services.browser.consent.get("root", &id).unwrap());
        let mut g = Gfx::new(1000, 700);
        d.render(&mut g, R::new(0.0, 0.0, 1000.0, 700.0), 1.0);
        let labels: Vec<String> = g.hits.iter().map(|h| h.label.clone()).collect();
        for want in ["1  Approve all actions on gemini.google.com for this session", "2  Approve one by one", "3  Deny"] {
            assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
        }
        assert_eq!(labels.iter().filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit())).count(), 3);
        assert_eq!(d.req.choices.iter().map(|c| c.label.clone()).collect::<Vec<_>>(), interact_choices("gemini.google.com").into_iter().map(|c| c.label).collect::<Vec<_>>());
        assert!(d.req.text.starts_with("Are you willing to let the assistant act on your behalf on gemini.google.com? It will be able to click, type and submit in your signed-in browser session. You can stop it at any time."));
        // answering ends the wait and is audited
        d.answer(&cx, Answer::OneByOne).unwrap();
        assert_eq!(cx.services.browser.consent.wait(&id, std::time::Duration::from_millis(10)), Some(Answer::OneByOne));
        assert!(d.answer(&cx, Answer::Deny).is_err(), "only once");
    }
}
