//! The Activity log and the read-only readout of one flow's assist (F-FLOW-33): what the agent proposed, published, rolled back and spent,
//! newest first, from the flow's book and its stored suggestions; plus the same facts as JSON for scripts and the assistant's tool. Pure
//! reading: nothing is written, no input value or secret is in the book, so none is in the output.

use serde_json::{json, Value};

use crate::assist_store::{AssistStore, Status};

/// How long ago, in words (`just now`, `5 min ago`, `3 h ago`, `2 days ago`).
pub fn ago(now_ms: i64, at_ms: i64) -> String {
    let s = (now_ms - at_ms).max(0) / 1000;
    match s {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", (s / 60).max(1)),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The log lines, newest first, at most `limit`: every event of the book and every suggestion (with how it stands now).
pub fn lines(store: &AssistStore, flow: &str, now_ms: i64, limit: usize) -> Vec<String> {
    let book = store.book(flow);
    let mut rows: Vec<(i64, String)> = Vec::new();
    for e in book.events.iter().rev() {
        let what = match e.kind.as_str() {
            "draft" => "the agent made a draft".to_string(),
            "publish" => "a new version was published".to_string(),
            "rollback" => "a version was rolled back".to_string(),
            "spend" => format!("the agent spent {}.{:02}", e.cents / 100, e.cents % 100),
            "pause" => "autopilot was paused".to_string(),
            "resume" => "autopilot was resumed".to_string(),
            other => other.to_string(),
        };
        rows.push((e.at_ms, if e.note.is_empty() { what } else { format!("{what} ({})", e.note) }));
    }
    for s in store.list(flow) {
        rows.push((s.created_ms, format!("suggestion {}: {} ({})", s.id, s.summary, s.status.label())));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().take(limit).map(|(at, t)| format!("{} · {t}", ago(now_ms, at))).collect()
}

/// The readout: mode of the book (paused, cooling), the suggestions waiting, the watch, the counters of today and the last lines.
pub fn readout(store: &AssistStore, flow: &str, now_ms: i64, limit: usize) -> Value {
    let all = store.list(flow);
    let waiting: Vec<Value> = all.iter().filter(|s| s.status.open()).map(|s| json!({"id": s.id, "status": s.status.label(), "summary": s.summary, "class": s.class.label()})).collect();
    let count = |st: Status| all.iter().filter(|s| s.status == st).count();
    let watch = store.watch(flow).map(|w| json!({"version": w.version, "previous": w.previous, "since_ms": w.since_ms, "suggestion": w.suggestion}));
    json!({
        "flow": flow,
        "paused": store.paused(flow),
        "cooling": store.cooling(flow),
        "waiting": waiting,
        "watch": watch,
        "counters": {
            "suggestions": all.len(),
            "approved": count(Status::Approved),
            "auto_published": count(Status::AutoPublished),
            "rejected": count(Status::Rejected),
            "rolled_back": count(Status::RolledBack),
            "publishes_today": store.publishes_today(flow),
            "drafts_today": store.drafts_today(flow),
            "cents_today": store.cents_today(flow),
        },
        "last": lines(store, flow, now_ms, limit),
    })
}
