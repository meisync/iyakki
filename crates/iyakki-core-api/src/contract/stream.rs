//! Large data (screenshots, logs, camera frames, files) never travels inside a call or an event: the call returns a handle and
//! the client reads it in chunks by offset, so a dropped link resumes where it stopped.

use super::context::CallContext;
use super::error::CallResult;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Screenshot,
    Log,
    CameraFrame,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamHandle {
    pub id: StreamId,
    pub kind: StreamKind,
    /// Total bytes when known (a log being written does not know).
    pub total: Option<u64>,
    /// The largest chunk the machine will send in one read.
    pub max_chunk: u32,
    pub media_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub stream: StreamId,
    pub offset: u64,
    pub data: Vec<u8>,
    /// True when this chunk reaches the end; reading at the end offset again returns an empty last chunk.
    pub last: bool,
}

/// Read side of the large-data handles. Reading checks the same scope as the call that produced the handle.
pub trait Streams: Send + Sync {
    fn read(&self, ctx: &CallContext, id: &StreamId, offset: u64, max_len: u32) -> CallResult<Chunk>;
    /// Free the handle early (handles also expire on their own).
    fn close(&self, ctx: &CallContext, id: &StreamId) -> CallResult<()>;
}
