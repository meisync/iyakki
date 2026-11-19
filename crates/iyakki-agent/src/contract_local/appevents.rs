//! `appevents.*` for a client device (F-SDK-202): the core's app events (`iyakki_core_api::AppEvents`, the same queue, `late` marker, scopes and
//! the person's switches as for a native app) read by sequence number over the contract, so a connected client is pushed them like settings and
//! flows events. A client is the app `client.<paired name>` of the person it is paired to; the core refuses what that app holds no scope for.
//!
//! Reading hands events out: the first read of a client starts it (what waited arrives, marked `late`), a read polls the queue and answers from
//! the delivered history after the sequence number the client gives (`Resume::After(n)`), so a client that lost its connection asks again with
//! the last number it saw and misses nothing that is still kept; `gap` says when the kept history no longer reaches back that far.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use iyakki_core_api::appevents::{AppEvent, MAX_HISTORY};
use iyakki_core_api::contract::*;
use iyakki_core_api::AppEvents;

use super::{guarded, Audit};

pub struct CoreAppEvents {
    events: Arc<dyn AppEvents>,
    audit: Arc<Audit>,
    /// The clients that have read at least once: `(person, app)`.
    started: Mutex<HashSet<(String, String)>>,
}

/// The one app a client device is, for the person it is paired to.
pub fn app_of(ctx: &CallContext) -> String {
    let n: String = ctx.identity.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(40).collect();
    format!("client.{n}")
}

fn body_of(e: &AppEvent) -> AppEventBody {
    AppEventBody { name: e.name.clone(), version: e.version, late: e.late, replayed: e.replayed, payload: e.payload.clone() }
}

impl CoreAppEvents {
    pub fn new(events: Arc<dyn AppEvents>) -> CoreAppEvents {
        CoreAppEvents { events, audit: Arc::new(Audit::default()), started: Mutex::new(HashSet::new()) }
    }

    pub fn audit_handle(&self) -> Arc<Audit> {
        self.audit.clone()
    }

    /// The feed of one person's client devices.
    pub fn for_person<'a>(&'a self, person: &'a str) -> PersonEvents<'a> {
        PersonEvents { core: self, person }
    }
}

pub struct PersonEvents<'a> {
    core: &'a CoreAppEvents,
    person: &'a str,
}

impl AppEventFeed for PersonEvents<'_> {
    fn register(&self, ctx: &CallContext, event: &str, filter: &str) -> CallResult<()> {
        guarded(&self.core.audit, ctx, "appevents.register", Scope::EVENTS_READ, Risk::Change)?;
        let r = self.core.events.register(self.person, &app_of(ctx), event, filter);
        self.core.audit.line(ctx, "appevents.register", Risk::Change, &match &r {
            Ok(()) => format!("ok: {event}"),
            Err(_) => format!("refused: {event}"),
        });
        r.map_err(|why| CallError::new(ErrorCode::Denied, "appevents.refused", &why))
    }

    fn events(&self, ctx: &CallContext, resume: Resume, limit: usize) -> CallResult<EventBatch<AppEventBody>> {
        guarded(&self.core.audit, ctx, "appevents.events", Scope::EVENTS_READ, Risk::Read)?;
        let app = app_of(ctx);
        let first = self.core.started.lock().unwrap_or_else(|e| e.into_inner()).insert((self.person.to_string(), app.clone()));
        // what waited (the first read) and what happened since: both are now in the delivered history
        let mut fresh = if first { self.core.events.start(self.person, &app) } else { Vec::new() };
        fresh.extend(self.core.events.poll(self.person, &app, MAX_HISTORY));
        let newest = fresh.iter().map(|e| e.seq).max();
        let (after, mut out): (u64, Vec<AppEvent>) = match resume {
            // from now on: what was just handed out is not news to a client that asked only for the latest
            Resume::Latest => {
                let last = newest.unwrap_or_else(|| self.core.events.replay(self.person, &app, 0).iter().map(|e| e.seq).max().unwrap_or(0));
                return Ok(EventBatch { events: vec![], next: last, gap: false });
            }
            Resume::After(n) => (n, self.core.events.replay(self.person, &app, n)),
        };
        out.sort_by_key(|e| e.seq);
        out.retain(|e| e.seq > after);
        let oldest = out.first().map(|e| e.seq);
        // the history keeps a bounded number of events: when the first one kept is not the next one the client expects, it missed some
        let gap = after > 0 && oldest.is_some_and(|o| o > after + 1 && !out.first().is_some_and(AppEvent::is_missed));
        out.truncate(limit);
        let next = out.last().map_or(after, |e| e.seq);
        Ok(EventBatch {
            events: out.iter().map(|e| Event { seq: e.seq, at_ms: e.at_ms, topic: "app".into(), body: body_of(e) }).collect(),
            next,
            gap,
        })
    }
}
