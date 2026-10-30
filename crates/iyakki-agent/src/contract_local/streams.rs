//! Large data behind handles: a call or an event never carries it, the client reads it in chunks by offset (a dropped link resumes
//! where it stopped). Reading checks the scope of the call that made the handle; handles expire and the total kept is bounded.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use iyakki_core_api::contract::*;

use super::{guarded, now_ms, Audit};

/// The largest chunk one read returns.
pub const MAX_CHUNK: u32 = 16 * 1024;
/// A handle is good for this long after it was made.
pub const TTL_MS: u64 = 10 * 60 * 1000;
const MAX_HANDLES: usize = 64;
const MAX_TOTAL: u64 = 64 << 20;

struct Item {
    kind: StreamKind,
    scope: String,
    data: Arc<Vec<u8>>,
    made_ms: u64,
}

pub struct CoreStreams {
    items: Mutex<HashMap<StreamId, Item>>,
    next: std::sync::atomic::AtomicU64,
    audit: Arc<Audit>,
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for CoreStreams {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreStreams {
    pub fn new() -> CoreStreams {
        CoreStreams::with_clock(Box::new(now_ms))
    }
    pub fn with_clock(clock: Box<dyn Fn() -> u64 + Send + Sync>) -> CoreStreams {
        CoreStreams { items: Mutex::new(HashMap::new()), next: std::sync::atomic::AtomicU64::new(0), audit: Arc::new(Audit::default()), clock }
    }
    pub fn audit(&self) -> Vec<String> {
        self.audit.all()
    }
    pub(crate) fn audit_handle(&self) -> &Audit {
        &self.audit
    }

    fn sweep(&self, items: &mut HashMap<StreamId, Item>) {
        let now = (self.clock)();
        items.retain(|_, i| now.saturating_sub(i.made_ms) < TTL_MS);
    }

    /// Keep `data` behind a new handle that a caller holding `scope` (or a local person) may read. Refused (`busy`) when too much
    /// is kept already: the oldest handle is not dropped silently.
    pub fn put(&self, kind: StreamKind, media_type: &str, scope: &str, data: Vec<u8>) -> CallResult<StreamHandle> {
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        self.sweep(&mut items);
        let total: u64 = items.values().map(|i| i.data.len() as u64).sum();
        if items.len() >= MAX_HANDLES || total + data.len() as u64 > MAX_TOTAL {
            return Err(CallError::new(ErrorCode::Busy, "streams.full", "Too much data is waiting to be read.").hint("Read or close the open streams first.").retry_after(1000));
        }
        let id = StreamId(format!("s{}", self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1));
        let len = data.len() as u64;
        items.insert(id.clone(), Item { kind, scope: scope.to_string(), data: Arc::new(data), made_ms: (self.clock)() });
        Ok(StreamHandle { id, kind, total: Some(len), max_chunk: MAX_CHUNK, media_type: media_type.to_string() })
    }
}

impl Streams for CoreStreams {
    fn read(&self, ctx: &CallContext, id: &StreamId, offset: u64, max_len: u32) -> CallResult<Chunk> {
        let (data, scope, kind) = {
            let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
            self.sweep(&mut items);
            let i = items.get(id).ok_or_else(|| CallError::new(ErrorCode::NotFound, "streams.unknown", "That stream is gone or never existed.").hint("Ask for the data again."))?;
            (i.data.clone(), i.scope.clone(), i.kind)
        };
        guarded(&self.audit, ctx, "streams.read", &scope, Risk::Read)?;
        let total = data.len() as u64;
        if offset > total {
            return Err(CallError::new(ErrorCode::InvalidInput, "streams.bad_offset", "That offset is past the end of the data."));
        }
        let want = max_len.clamp(1, MAX_CHUNK) as u64;
        let end = (offset + want).min(total);
        let chunk = Chunk { stream: id.clone(), offset, data: data[offset as usize..end as usize].to_vec(), last: end >= total };
        if chunk.last {
            self.audit.line(ctx, &format!("streams.read {} ({})", id.0, format!("{kind:?}").to_lowercase()), Risk::Read, "ok: reached the end");
        }
        Ok(chunk)
    }

    fn close(&self, ctx: &CallContext, id: &StreamId) -> CallResult<()> {
        let scope = {
            let items = self.items.lock().unwrap_or_else(|e| e.into_inner());
            items.get(id).map(|i| i.scope.clone())
        };
        let Some(scope) = scope else { return Ok(()) };
        guarded(&self.audit, ctx, "streams.close", &scope, Risk::Read)?;
        self.items.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        Ok(())
    }
}
