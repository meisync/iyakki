//! The dialog that asks the person about the assistant's use of their browser (docs/BROWSER-AGENT.md): may it look at a site, are
//! you willing to let it act for you there (approve all / one by one / deny), may it do this one thing, can you do something
//! only you can (a code, a captcha) and press Continue. The questions are held by `iyakki_shared::browserconsent`; the INBOX
//! (Approvals page) and the open chat both show them with this dialog, so an answer in either place ends the wait.
//!
//! The drawing and the key map are the tile crate's (`iyakki_tile_inbox::consent`, on the plain mirrors of `iyakki-tile-api`); this file
//! converts between the agent's request and the mirror, and gives the answer to the waiting tool (the only part that needs the shell).

use iyakki_shared::browserconsent::{Answer, ConsentChoice, ConsentKind, ConsentRequest};
use iyakki_tile_api::inbox as api;

use super::AppCx;
use crate::gfx::Gfx;
use crate::ui::{Key, R};

pub fn kind_to_api(k: ConsentKind) -> api::ConsentKind {
    match k {
        ConsentKind::Look => api::ConsentKind::Look,
        ConsentKind::Interact => api::ConsentKind::Interact,
        ConsentKind::Step => api::ConsentKind::Step,
        ConsentKind::Handover => api::ConsentKind::Handover,
        ConsentKind::Permission => api::ConsentKind::Permission,
    }
}

pub fn kind_from_api(k: api::ConsentKind) -> ConsentKind {
    match k {
        api::ConsentKind::Look => ConsentKind::Look,
        api::ConsentKind::Interact => ConsentKind::Interact,
        api::ConsentKind::Step => ConsentKind::Step,
        api::ConsentKind::Handover => ConsentKind::Handover,
        api::ConsentKind::Permission => ConsentKind::Permission,
    }
}

pub fn answer_to_api(a: Answer) -> api::Answer {
    match a {
        Answer::Allow => api::Answer::Allow,
        Answer::AllowSession => api::Answer::AllowSession,
        Answer::Deny => api::Answer::Deny,
        Answer::ApproveAll => api::Answer::ApproveAll,
        Answer::OneByOne => api::Answer::OneByOne,
        Answer::Continue => api::Answer::Continue,
        Answer::AllowOnce => api::Answer::AllowOnce,
        Answer::AlwaysAllow => api::Answer::AlwaysAllow,
        Answer::NotNow => api::Answer::NotNow,
        Answer::Block => api::Answer::Block,
    }
}

pub fn answer_from_api(a: api::Answer) -> Answer {
    match a {
        api::Answer::Allow => Answer::Allow,
        api::Answer::AllowSession => Answer::AllowSession,
        api::Answer::Deny => Answer::Deny,
        api::Answer::ApproveAll => Answer::ApproveAll,
        api::Answer::OneByOne => Answer::OneByOne,
        api::Answer::Continue => Answer::Continue,
        api::Answer::AllowOnce => Answer::AllowOnce,
        api::Answer::AlwaysAllow => Answer::AlwaysAllow,
        api::Answer::NotNow => Answer::NotNow,
        api::Answer::Block => Answer::Block,
    }
}

pub fn request_to_api(r: &ConsentRequest) -> api::ConsentRequest {
    api::ConsentRequest {
        id: r.id.clone(),
        user: r.user.clone(),
        kind: kind_to_api(r.kind),
        domain: r.domain.clone(),
        title: r.title.clone(),
        text: r.text.clone(),
        detail: r.detail.clone(),
        choices: r.choices.iter().map(|c| api::ConsentChoice { answer: answer_to_api(c.answer), label: c.label.clone() }).collect(),
        since: r.since,
        payload: r.payload.clone(),
        quiet: r.quiet,
    }
}

pub fn request_from_api(r: &api::ConsentRequest) -> ConsentRequest {
    ConsentRequest {
        id: r.id.clone(),
        user: r.user.clone(),
        kind: kind_from_api(r.kind),
        domain: r.domain.clone(),
        title: r.title.clone(),
        text: r.text.clone(),
        detail: r.detail.clone(),
        choices: r.choices.iter().map(|c| ConsentChoice { answer: answer_from_api(c.answer), label: c.label.clone() }).collect(),
        since: r.since,
        payload: r.payload.clone(),
        quiet: r.quiet,
    }
}

#[derive(Clone, Debug)]
pub struct ConsentDialog {
    pub req: ConsentRequest,
    tile: iyakki_tile_inbox::ConsentDialog,
}

impl ConsentDialog {
    pub fn new(req: ConsentRequest) -> ConsentDialog {
        let tile = iyakki_tile_inbox::ConsentDialog::new(request_to_api(&req));
        ConsentDialog { req, tile }
    }

    /// The answer a key stands for: 1, 2, 3 choose by position; Esc is the "no" (Deny or Cancel) of the question.
    pub fn answer_of(&self, k: &Key) -> Option<Answer> {
        self.tile.answer_of(k).map(answer_from_api)
    }

    pub fn render(&self, g: &mut Gfx, area: R, s: f32) {
        self.tile.render(g, area, s)
    }

    /// Give the answer to the waiting tool; audited by what it allows, never by the page's content.
    pub fn answer(&self, cx: &AppCx, a: Answer) -> Result<(), String> {
        let r = cx.browser_consent().resolve(&cx.session.user, &self.req.id, a);
        if r.is_ok() && self.req.kind == ConsentKind::Permission {
            // the person's own answer to a flow's permission request: carried out by the flows engine (audited there, without secrets)
            return match cx.flows_facet().answer_permission(&cx.session.user, &cx.session.policy, &self.req, a) {
                Ok(_) => Ok(()),
                Err(e) => {
                    // nothing was done: the request comes back so the person can answer again
                    if a != Answer::NotNow {
                        let _ = cx.browser_consent().create_permission(&self.req.user, &self.req.domain, &self.req.title, &self.req.text, &self.req.payload, self.req.since);
                    }
                    Err(e)
                }
            };
        }
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
                (ConsentKind::Permission, _) => "permission answered",
            };
            cx.audit.record(&cx.session.user, "browser.agent.answer", &format!("{} {}", self.req.domain, what));
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_shared::browserconsent::{interact_choices, willingness_text};

    fn req(kind: ConsentKind) -> ConsentRequest {
        let cx = cx_with_admin_store("consentdlg");
        let id = cx.browser_consent().create("root", kind, "a.test", &willingness_text("a.test", None), "click button \"Send\"", 1).unwrap();
        cx.browser_consent().get("root", &id).unwrap()
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
        let id = cx.browser_consent().create("root", ConsentKind::Interact, "gemini.google.com", &willingness_text("gemini.google.com", None), "", 1).unwrap();
        let d = ConsentDialog::new(cx.browser_consent().get("root", &id).unwrap());
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
        assert_eq!(cx.browser_consent().wait(&id, std::time::Duration::from_millis(10)), Some(Answer::OneByOne));
        assert!(d.answer(&cx, Answer::Deny).is_err(), "only once");
    }

    #[test]
    fn the_mirrors_of_the_tile_api_round_trip_every_kind_answer_and_request() {
        for k in [ConsentKind::Look, ConsentKind::Interact, ConsentKind::Step, ConsentKind::Handover, ConsentKind::Permission] {
            assert_eq!(kind_from_api(kind_to_api(k)), k);
        }
        for a in [Answer::Allow, Answer::AllowSession, Answer::Deny, Answer::ApproveAll, Answer::OneByOne, Answer::Continue, Answer::AllowOnce, Answer::AlwaysAllow, Answer::NotNow, Answer::Block] {
            assert_eq!(answer_from_api(answer_to_api(a)), a);
        }
        let r = ConsentRequest { id: "i".into(), user: "u".into(), kind: ConsentKind::Permission, domain: "d".into(), title: "t".into(), text: "x".into(), detail: "y".into(), choices: vec![ConsentChoice { answer: Answer::Block, label: "Block".into() }], since: 7, payload: "p".into(), quiet: true };
        assert_eq!(request_from_api(&request_to_api(&r)), r);
    }
}
