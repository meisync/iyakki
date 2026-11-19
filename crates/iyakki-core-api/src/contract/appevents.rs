//! App events for a client device (F-SDK-202): the same queue, `late` marker and scopes as for a native app, read by sequence number so a
//! client resumes where it stopped instead of asking again. A client listens as the app `client.<paired name>`; what it may hear is what the
//! core's scopes grant that app (the person approves it, as for any installed app). Payloads are the catalogue's names and numbers, never a
//! secret.

use super::context::CallContext;
use super::error::CallResult;
use super::event::{EventBatch, Resume};
use crate::appevents::Payload;
use serde::{Deserialize, Serialize};

/// One app event as a client reads it (the event's own sequence number is the stream's).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEventBody {
    pub name: String,
    pub version: u32,
    pub late: bool,
    pub replayed: bool,
    pub payload: Payload,
}

pub trait AppEventFeed: Send + Sync {
    /// Listen for an event (`filter` is the prefix some events take, else empty). Refused with a sentence when the app holds no scope for it.
    fn register(&self, ctx: &CallContext, event: &str, filter: &str) -> CallResult<()>;
    /// Events after `resume`, oldest first, at most `limit`. Reading them hands them out: they wait no more, and a later read after the
    /// last sequence number seen continues. `gap` is true when events the client missed were dropped (the `missed` marker says how many).
    fn events(&self, ctx: &CallContext, resume: Resume, limit: usize) -> CallResult<EventBatch<AppEventBody>>;
}
