//! What the web routes call: a person's own notifications. The person is always the signed-in account; nothing here takes a name from
//! the request. Counts are read from the stored INBOX, never from memory.

use super::service::Hub;
use serde_json::{json, Value};

#[derive(Debug, PartialEq, Eq)]
pub struct ApiErr {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

fn e(status: u16, code: &'static str, m: &str) -> ApiErr {
    ApiErr { status, code, message: m.into() }
}

fn map(m: String) -> ApiErr {
    if m.contains("no such") { e(404, "not_found", "no such notification") } else { e(503, "io", "the notifications could not be read or saved") }
}

pub const MAX_LIMIT: usize = 200;
pub const DEFAULT_LIMIT: usize = 50;

fn view(n: &crate::inbox::Notification, now: i64) -> Value {
    let mut v = serde_json::to_value(n).unwrap_or(Value::Null);
    v["snoozed"] = json!(n.is_snoozed(now));
    v
}

/// Newest first. `before` is a time cursor: only notes older than it. Snoozed notes are left out unless `snoozed` is asked for.
pub fn list(hub: &Hub, user: &str, unread: bool, snoozed: bool, limit: usize, before: Option<i64>, now: i64) -> Value {
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit.min(MAX_LIMIT) };
    let all = hub.inbox.list(user);
    let mut rows: Vec<&crate::inbox::Notification> = all.iter().filter(|n| (!unread || !n.read) && (snoozed || !n.is_snoozed(now)) && before.is_none_or(|b| n.time < b)).collect();
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = if more { rows.last().map(|n| n.time) } else { None };
    json!({"notifications": rows.iter().map(|n| view(n, now)).collect::<Vec<_>>(), "next_before": next})
}

/// The bell: unread (not snoozed), approvals waiting (tool calls the person must answer and tenant requests they may decide) and
/// overdue tasks. `red` while an approval waits.
pub fn count(hub: &Hub, user: &str, now: i64, approvals: usize) -> Value {
    let c = hub.inbox.counts(user, now);
    json!({"unread": c.unread, "attention": c.attention, "approvals": approvals, "overdue": c.overdue, "total": c.unread + approvals + c.overdue, "red": approvals > 0})
}

pub fn read(hub: &Hub, user: &str, id: &str, read: bool, now: i64) -> Result<Value, ApiErr> {
    hub.inbox.mark_read(user, id, read).map_err(map)?;
    hub.bus.publish(user, &super::events::event("count", json!({"unread": hub.inbox.counts(user, now).unread})));
    Ok(json!({"ok": true}))
}

pub fn read_all(hub: &Hub, user: &str, now: i64) -> Result<Value, ApiErr> {
    let n = hub.inbox.mark_all_read(user).map_err(map)?;
    hub.bus.publish(user, &super::events::event("count", json!({"unread": hub.inbox.counts(user, now).unread})));
    Ok(json!({"ok": true, "marked": n}))
}

/// `until` (seconds since 1970) or `minutes`; at most 30 days ahead.
pub fn snooze(hub: &Hub, user: &str, id: &str, body: &Value, now: i64) -> Result<Value, ApiErr> {
    let until = match (body.get("until").and_then(|x| x.as_i64()), body.get("minutes").and_then(|x| x.as_i64())) {
        (Some(u), _) => u,
        (None, Some(m)) => now + m.saturating_mul(60),
        _ => return Err(e(400, "invalid_input", "send {\"minutes\":N} or {\"until\":seconds}")),
    };
    if until <= now || until > now + 30 * 86_400 {
        return Err(e(400, "invalid_input", "snooze between now and 30 days"));
    }
    hub.inbox.snooze(user, id, until).map_err(map)?;
    hub.bus.publish(user, &super::events::event("count", json!({"unread": hub.inbox.counts(user, now).unread})));
    Ok(json!({"ok": true, "until": until}))
}

pub fn ack(hub: &Hub, user: &str, id: &str, now: i64) -> Result<Value, ApiErr> {
    let escalation = hub.ack(user, id, now as u64).map_err(map)?;
    Ok(json!({"ok": true, "escalation_stopped": escalation}))
}

pub fn delete(hub: &Hub, user: &str, id: &str, now: i64) -> Result<Value, ApiErr> {
    hub.inbox.delete(user, id).map_err(map)?;
    hub.bus.publish(user, &super::events::event("count", json!({"unread": hub.inbox.counts(user, now).unread})));
    Ok(json!({"ok": true}))
}
