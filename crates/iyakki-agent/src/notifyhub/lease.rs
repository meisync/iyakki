//! A lease on the tenant's volume: one replica at a time runs the workers (the same idea as the scheduler's lease; when the cloud
//! flows module offers its own, this file is what to replace). The holder renews before `ttl` runs out; a holder that stopped loses
//! it when the time passes. The decision is made under an exclusive file lock so two processes cannot both take it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default)]
struct Rec {
    holder: String,
    expires: u64,
}

pub struct Lease {
    path: PathBuf,
}

impl Lease {
    pub fn new(path: PathBuf) -> Lease {
        Lease { path }
    }

    /// True when `holder` holds the lease after this call (it took it, or renewed it).
    pub fn try_acquire(&self, holder: &str, now: u64, ttl: u64) -> bool {
        let Some(dir) = self.path.parent() else { return false };
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
        let Ok(guard) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.path.with_extension("lock")) else { return false };
        if guard.lock().is_err() {
            return false;
        }
        let cur: Rec = std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let free = cur.holder.is_empty() || cur.expires <= now || cur.holder == holder;
        let won = free && iyakki_schema::write_atomic(&self.path, serde_json::to_string(&Rec { holder: holder.into(), expires: now + ttl }).unwrap_or_default().as_bytes(), 0o600).is_ok();
        let _ = guard.unlock();
        won
    }

    pub fn release(&self, holder: &str) {
        let cur: Rec = std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if cur.holder == holder {
            let _ = iyakki_schema::write_atomic(&self.path, serde_json::to_string(&Rec::default()).unwrap_or_default().as_bytes(), 0o600);
        }
    }
}
