//! The leader lease: one replica at a time runs a tenant's schedules.
//!
//! A small JSON file on the tenant volume (`<data>/cloud/flows/lease.json`) names the holder, an epoch that grows at every change of
//! holder (a fencing number) and the time of the holder's last heartbeat. Taking it over is allowed when the last heartbeat is older
//! than the lease time. Writes are atomic renames; after writing, the writer reads the file back and only believes it holds the lease
//! if its own name is in it. The holder renews every third of the lease time and stops all scheduling the moment a renewal finds
//! another name.
//!
//! Honest limits: this is a lease on a file, not a consensus protocol. On a local or block-device volume (ReadWriteOnce, the default)
//! a rename is atomic and the read-back is current. On a network file system with client caching (NFS) a read can be stale for some
//! seconds, so two replicas could both believe they lead for up to one renewal interval; keep the lease time well above the cache
//! time (default 30 s) and prefer ReadWriteOnce volumes. The shipped manifest runs one replica with `Recreate`, so overlap exists only
//! while an old pod is still shutting down.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_TTL_SECS: u64 = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Body {
    pub holder: String,
    pub epoch: u64,
    pub beat: u64,
    pub ttl: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Acquire {
    /// This replica leads; the number is the fencing epoch.
    Held(u64),
    /// Somebody else does, until `expires` (seconds).
    Busy { holder: String, expires: u64 },
}

pub struct Lease {
    path: PathBuf,
    holder: String,
    ttl: u64,
}

impl Lease {
    pub fn new(flows_dir: &Path, holder: &str, ttl_secs: u64) -> Lease {
        Lease { path: flows_dir.join("lease.json"), holder: holder.to_string(), ttl: ttl_secs.clamp(5, 3600) }
    }

    pub fn holder_name(&self) -> &str {
        &self.holder
    }

    pub fn ttl(&self) -> u64 {
        self.ttl
    }

    pub fn read(&self) -> Option<Body> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok())
    }

    fn write(&self, b: &Body) -> bool {
        super::save_json(&self.path, b).is_ok()
    }

    pub fn try_acquire(&self, now: u64) -> Acquire {
        let cur = self.read();
        let epoch = match &cur {
            Some(b) if b.holder == self.holder => b.epoch,
            Some(b) if b.beat.saturating_add(b.ttl) > now => return Acquire::Busy { holder: b.holder.clone(), expires: b.beat + b.ttl },
            Some(b) => b.epoch + 1,
            None => 1,
        };
        let mine = Body { holder: self.holder.clone(), epoch, beat: now, ttl: self.ttl };
        if !self.write(&mine) {
            return Acquire::Busy { holder: "(the volume is not writable)".into(), expires: now + self.ttl };
        }
        match self.read() {
            Some(b) if b.holder == self.holder => Acquire::Held(b.epoch),
            Some(b) => Acquire::Busy { holder: b.holder, expires: b.beat + b.ttl },
            None => Acquire::Busy { holder: "?".into(), expires: now + self.ttl },
        }
    }

    /// Renew. `Err` means the lease is gone (another holder, or it ran out and was taken): stop leading at once.
    pub fn heartbeat(&self, now: u64) -> Result<u64, String> {
        match self.read() {
            Some(b) if b.holder == self.holder => {
                let nb = Body { beat: now, ttl: self.ttl, ..b };
                if self.write(&nb) { Ok(nb.epoch) } else { Err("the lease file could not be written".into()) }
            }
            Some(b) => Err(format!("the lease is held by {}", b.holder)),
            None => Err("the lease file is gone".into()),
        }
    }

    /// Give it up at a clean stop so the next replica need not wait for the time to run out.
    pub fn release(&self) {
        if let Some(b) = self.read() {
            if b.holder == self.holder {
                let _ = self.write(&Body { beat: 0, ..b });
            }
        }
    }
}
