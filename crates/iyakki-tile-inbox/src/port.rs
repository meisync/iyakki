//! What the INBOX tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile names no `Services`, no session and no audit log: it asks an [`InboxPort`] for the signed-in person, whether they manage the system,
//! the notifications and tasks (the 17 calls of the inbox store), the calendar, the vault's secret requests, the browser questions, the device
//! watch, the pending tool approvals, the picture decoder and the audit line. The real port (`CxPort` in the shell crate, `inbox_port.rs`) is the
//! only place that reads the session and calls the services; a fake hands the tile in-memory stores.

use iyakki_inbox_model::{Level, Notification, Priority, Status, Task, TaskPatch, TaskSort};
use iyakki_tile_api::calendar::CalendarSource;
use iyakki_tile_api::inbox::{Answer, ConsentRequest, PendingApproval, SecretRequest};
use iyakki_uikit::tilekit::Pixmap;

pub trait InboxPort {
    /// The signed-in person.
    fn user(&self) -> String;
    /// The person manages the system (administrators): may trust new devices, sees group details.
    fn is_admin(&self) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);

    // ---- notifications and tasks (the inbox store, one person at a time)
    /// Group an older file and drop what is past its keeping time.
    fn maintain(&self, user: &str, now: i64) -> Result<(), String>;
    /// What was done to the file when it was opened (said once).
    fn take_notice(&self, user: &str) -> Option<String>;
    fn list(&self, user: &str) -> Vec<Notification>;
    /// The small picture of a note (JPEG bytes), when it has one.
    fn picture(&self, user: &str, id: &str) -> Option<Vec<u8>>;
    fn mark_read(&self, user: &str, id: &str, read: bool) -> Result<(), String>;
    fn mark_all_read(&self, user: &str) -> Result<usize, String>;
    fn flag(&self, user: &str, id: &str, flagged: bool) -> Result<(), String>;
    fn snooze(&self, user: &str, id: &str, until: i64) -> Result<(), String>;
    fn level(&self, user: &str, source: &str) -> Level;
    fn set_level(&self, user: &str, source: &str, level: Level) -> Result<(), String>;
    fn delete(&self, user: &str, id: &str) -> Result<(), String>;
    fn clear_read(&self, user: &str) -> Result<usize, String>;
    fn tasks(&self, user: &str, sort: TaskSort) -> Vec<Task>;
    #[allow(clippy::too_many_arguments)]
    fn add_task(&self, user: &str, title: &str, notes: &str, due: Option<i64>, priority: Priority, by: &str, now: i64) -> Result<Task, String>;
    fn update_task(&self, user: &str, id: &str, patch: TaskPatch, now: i64) -> Result<Task, String>;
    fn move_task(&self, user: &str, id: &str, status: Status, now: i64) -> Result<Task, String>;
    fn delete_task(&self, user: &str, id: &str) -> Result<(), String>;

    // ---- the calendar, in the machine's time zone
    fn calendar(&self) -> Box<dyn CalendarSource + '_>;

    // ---- what waits for an answer
    /// The secrets the assistant asked for, and the person's answer (the value goes to the vault under `name`, which is returned).
    fn secret_pending(&self, user: &str) -> Vec<SecretRequest>;
    fn secret_get(&self, user: &str, id: &str) -> Option<SecretRequest>;
    fn answer_secret_request(&self, user: &str, id: &str, name: &str, value: &str) -> Result<String, String>;
    fn cancel_secret_request(&self, user: &str, id: &str) -> Result<(), String>;
    /// The questions about the assistant in the person's browser, and the answer (the core, the flows facet and the audit log are told).
    fn consent_pending(&self, user: &str) -> Vec<ConsentRequest>;
    fn consent_get(&self, user: &str, id: &str) -> Option<ConsentRequest>;
    fn consent_answer(&self, req: &ConsentRequest, answer: Answer) -> Result<(), String>;
    /// Trust the device with this address (administrators only; the tile checks).
    fn devwatch_trust(&self, mac: &str) -> bool;
    /// The tool calls of the person's agent runs that wait for an answer, and the answer.
    fn approvals(&self, user: &str) -> Vec<PendingApproval>;
    fn decide_approval(&self, user: &str, run_id: &str, call_id: &str, approve: bool) -> Result<PendingApproval, String>;

    // ---- pictures
    /// A small JPEG (a motion event's thumbnail) as a pixmap; `None` when it cannot be read.
    fn decode_thumb(&self, jpeg: &[u8]) -> Option<Pixmap>;
}
