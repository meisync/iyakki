//! The relay server of F-CORE-17 / F-CORE-33 / F-FLEET-10 (docs/RELAY.md).
//!
//! A relay is a mailbox, not a computer. It keeps sealed envelopes (`iyakki-agent`, `devmode/relay.rs`) for a mailbox id until they are
//! fetched or until they expire (24 hours at most) and never opens one: it holds no key and has no code that reads a body. It can see
//! who posts to which mailbox, when, how big, and from which network address; nothing else.
//!
//! * [`store`]: the queues, limits and expiry, with an injected [`Clock`] (no sleeps in its tests).
//! * [`http`]: the three routes, parsed and answered by hand over any `Read + Write`.
//! * [`tls`]: the certificate and key given on the command line, served with rustls.
//! * [`server`]: the accept loop with a bound listener handed in (so tests keep their port).

pub mod http;
pub mod server;
pub mod store;
pub mod tls;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Milliseconds since the Unix epoch, from whatever the caller injects.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The real clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }
}

/// A clock that only moves when told to (tests).
#[derive(Clone, Default)]
pub struct ManualClock(pub Arc<AtomicU64>);

impl ManualClock {
    pub fn at(ms: u64) -> ManualClock {
        ManualClock(Arc::new(AtomicU64::new(ms)))
    }
    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
