//! The real INBOX port: what the tile (`iyakki-tile-inbox`) asks of the rest of the system, answered over the shell's context (the app standard,
//! docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10). This is the only place of the INBOX that reads the session and calls the services;
//! it also converts between the agent's shapes (`iyakki_shared::runs::PendingApproval`, `secretreq::SecretRequest`, the browser questions) and
//! the plain mirrors of `iyakki-tile-api` (a shell test keeps the copies in agreement).

use iyakki_shared::inbox::{Level, Notification, Priority, Status, Task, TaskPatch, TaskSort};
use iyakki_shared::runs::PendingApproval;
use iyakki_tile_api::calendar::CalendarSource;
use iyakki_tile_api::inbox as api;
use iyakki_tile_inbox::InboxPort;
use iyakki_uikit::tilekit::Pixmap;

use super::browser_consent::{answer_from_api, request_from_api, request_to_api, ConsentDialog};
use super::calendar::OwnedCalendar;
use super::AppCx;

pub fn approval_to_api(a: PendingApproval) -> api::PendingApproval {
    api::PendingApproval { run_id: a.run_id, user: a.user, prompt: a.prompt, call_id: a.call_id, tool: a.tool, summary: a.summary, detail: a.detail, risk: a.risk, since: a.since }
}

pub fn approval_from_api(a: api::PendingApproval) -> PendingApproval {
    PendingApproval { run_id: a.run_id, user: a.user, prompt: a.prompt, call_id: a.call_id, tool: a.tool, summary: a.summary, detail: a.detail, risk: iyakki_shared::runs::risk_word(a.risk), since: a.since }
}

fn secret_to_api(r: iyakki_shared::secretreq::SecretRequest) -> api::SecretRequest {
    api::SecretRequest { id: r.id, user: r.user, name: r.name, purpose: r.purpose, kind: r.kind, since: r.since }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl InboxPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn is_admin(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn maintain(&self, user: &str, now: i64) -> Result<(), String> {
        self.0.inbox().maintain(user, now)
    }
    fn take_notice(&self, user: &str) -> Option<String> {
        self.0.inbox().take_notice(user)
    }
    fn list(&self, user: &str) -> Vec<Notification> {
        self.0.inbox().list(user)
    }
    fn picture(&self, user: &str, id: &str) -> Option<Vec<u8>> {
        self.0.inbox().picture(user, id)
    }
    fn mark_read(&self, user: &str, id: &str, read: bool) -> Result<(), String> {
        self.0.inbox().mark_read(user, id, read)
    }
    fn mark_all_read(&self, user: &str) -> Result<usize, String> {
        self.0.inbox().mark_all_read(user)
    }
    fn flag(&self, user: &str, id: &str, flagged: bool) -> Result<(), String> {
        self.0.inbox().flag(user, id, flagged)
    }
    fn snooze(&self, user: &str, id: &str, until: i64) -> Result<(), String> {
        self.0.inbox().snooze(user, id, until)
    }
    fn level(&self, user: &str, source: &str) -> Level {
        self.0.inbox().level(user, source)
    }
    fn set_level(&self, user: &str, source: &str, level: Level) -> Result<(), String> {
        self.0.inbox().set_level(user, source, level)
    }
    fn delete(&self, user: &str, id: &str) -> Result<(), String> {
        self.0.inbox().delete(user, id)
    }
    fn clear_read(&self, user: &str) -> Result<usize, String> {
        self.0.inbox().clear_read(user)
    }
    fn tasks(&self, user: &str, sort: TaskSort) -> Vec<Task> {
        self.0.inbox().tasks(user, sort)
    }
    fn add_task(&self, user: &str, title: &str, notes: &str, due: Option<i64>, priority: Priority, by: &str, now: i64) -> Result<Task, String> {
        self.0.inbox().add_task(user, title, notes, due, priority, by, now)
    }
    fn update_task(&self, user: &str, id: &str, patch: TaskPatch, now: i64) -> Result<Task, String> {
        self.0.inbox().update_task(user, id, patch, now)
    }
    fn move_task(&self, user: &str, id: &str, status: Status, now: i64) -> Result<Task, String> {
        self.0.inbox().move_task(user, id, status, now)
    }
    fn delete_task(&self, user: &str, id: &str) -> Result<(), String> {
        self.0.inbox().delete_task(user, id)
    }
    fn calendar(&self) -> Box<dyn CalendarSource + '_> {
        Box::new(OwnedCalendar { store: self.0.calendar(), zone: iyakki_clock::Zone::system() })
    }
    fn secret_pending(&self, user: &str) -> Vec<api::SecretRequest> {
        self.0.secret_requests().pending(user).into_iter().map(secret_to_api).collect()
    }
    fn secret_get(&self, user: &str, id: &str) -> Option<api::SecretRequest> {
        self.0.secret_requests().get(user, id).map(secret_to_api)
    }
    fn answer_secret_request(&self, user: &str, id: &str, name: &str, value: &str) -> Result<String, String> {
        self.0.vault().answer_secret_request(user, id, name, value)
    }
    fn cancel_secret_request(&self, user: &str, id: &str) -> Result<(), String> {
        self.0.vault().cancel_secret_request(user, id)
    }
    fn consent_pending(&self, user: &str) -> Vec<api::ConsentRequest> {
        self.0.browser_consent().pending(user).iter().map(request_to_api).collect()
    }
    fn consent_get(&self, user: &str, id: &str) -> Option<api::ConsentRequest> {
        self.0.browser_consent().get(user, id).map(|r| request_to_api(&r))
    }
    fn consent_answer(&self, req: &api::ConsentRequest, answer: api::Answer) -> Result<(), String> {
        ConsentDialog::new(request_from_api(req)).answer(self.0, answer_from_api(answer))
    }
    fn devwatch_trust(&self, mac: &str) -> bool {
        self.0.devwatch().trust(mac)
    }
    fn approvals(&self, user: &str) -> Vec<api::PendingApproval> {
        self.0.services.approvals(user).into_iter().map(approval_to_api).collect()
    }
    fn decide_approval(&self, user: &str, run_id: &str, call_id: &str, approve: bool) -> Result<api::PendingApproval, String> {
        self.0.services.decide_approval(user, run_id, call_id, approve).map(approval_to_api)
    }
    fn decode_thumb(&self, jpeg: &[u8]) -> Option<Pixmap> {
        let b = iyakki_web::jpeg::decode(jpeg).ok()?;
        crate::camera::pixels::pixmap(b.width, b.height, b.rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approval_and_its_mirror_round_trip_and_an_unknown_risk_word_reads_as_high() {
        for risk in ["low", "medium", "high"] {
            let a = PendingApproval { run_id: "r".into(), user: "u".into(), prompt: "p".into(), call_id: "c".into(), tool: "t".into(), summary: "s".into(), detail: "d".into(), risk: iyakki_shared::runs::risk_word(risk), since: 3 };
            let back = approval_from_api(approval_to_api(a.clone()));
            assert_eq!((back.run_id, back.risk, back.since, back.detail), (a.run_id, a.risk, a.since, a.detail));
        }
        let odd = api::PendingApproval { run_id: "r".into(), user: "u".into(), prompt: "".into(), call_id: "c".into(), tool: "t".into(), summary: "".into(), detail: "".into(), risk: "urgent", since: 0 };
        assert_eq!(approval_from_api(odd).risk, "high");
    }
}
